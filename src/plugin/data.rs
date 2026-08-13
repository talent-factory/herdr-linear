//! Composes existing `LinearClient` calls into what the plugin needs: the
//! authenticated viewer's open assigned issues, the current project's open issues, and
//! (TF-579) a resolved team's open issues.

use crate::plugin::query::{FilterTerm, PriorityOp};
use crate::plugin::{config, repo};
use crate::{Error, Issue, LinearClient, Project, Result, Team};
use serde_json::{json, Value};

/// Page size used when paginating `get_issues`/`get_projects` to completion (see
/// [`fetch_issues_paginated`]/[`fetch_all_projects`]). 50 matches `LinearClient`'s own
/// default; Linear's API caps a single page at 250 regardless of what's requested.
const ISSUE_PAGE_SIZE: i32 = 50;
const PROJECT_PAGE_SIZE: i32 = 250;

/// Page size for `get_teams` when resolving which team to use for the Team Issues
/// view: matches `PROJECT_PAGE_SIZE`'s use of the API's own single-page cap, since
/// (like [`fetch_all_projects`]) every team in the workspace is needed to tell
/// "exactly one" from "more than one" — a truncated first page could wrongly report
/// an ambiguous workspace as unambiguous.
const TEAM_PAGE_SIZE: i32 = 250;

/// Hang/cost guard on the pagination loops below: stop after this many pages even if
/// Linear still reports more, logging a warning rather than fetching indefinitely. Chosen
/// generously above any real project's open-issue count or workspace's project count —
/// hitting it means something (a filter bug, a runaway workspace) — but it's a fetch of
/// a few thousand records at worst, not an unbounded one.
const MAX_PAGES: u32 = 20;

/// A Linear issue filter matching open (not completed, not canceled) issues assigned to
/// `user_id`, additionally narrowed by `filter_terms` (TF-615's parsed `priority:`/
/// `state:`/`label:` terms — see `merge_filter_terms` below). "Open" is expressed as an
/// exclusion (`nin`) rather than an allowlist of the non-terminal state types
/// (`triage`/`backlog`/`unstarted`/`started`), so it can't silently drop issues in a
/// state type this code doesn't know about — mirrors [`project_open_filter`]. An empty
/// `filter_terms` reproduces the exact JSON this function returned before TF-616 — no
/// behavior change for callers that don't have a query.
pub fn assignee_open_filter(user_id: &str, filter_terms: &[FilterTerm]) -> Value {
    let base = json!({
        "assignee": { "id": { "eq": user_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    });
    merge_filter_terms(base, filter_terms)
}

/// A Linear issue filter matching open (not completed, not canceled) issues in
/// `project_id`, additionally narrowed by `filter_terms` (see [`assignee_open_filter`]
/// for the empty-slice/no-behavior-change guarantee). "Open" is expressed as an
/// exclusion (`nin`) rather than an allowlist of the non-terminal state types
/// (`triage`/`backlog`/`unstarted`/`started`), so it can't silently drop issues in a
/// state type this code doesn't know about — mirrors [`assignee_open_filter`].
pub fn project_open_filter(project_id: &str, filter_terms: &[FilterTerm]) -> Value {
    let base = json!({
        "project": { "id": { "eq": project_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    });
    merge_filter_terms(base, filter_terms)
}

/// A Linear issue filter matching open (not completed, not canceled) issues in
/// `team_id`, additionally narrowed by `filter_terms` (see [`assignee_open_filter`] for
/// the empty-slice/no-behavior-change guarantee). "Open" is expressed as an exclusion
/// (`nin`) rather than an allowlist of the non-terminal state types
/// (`triage`/`backlog`/`unstarted`/`started`), so it can't silently drop issues in a
/// state type this code doesn't know about — mirrors [`assignee_open_filter`]/
/// [`project_open_filter`]. See [`fetch_team_issues`] for why this is composed with
/// `get_issues` instead of delegating to `get_team_issues`.
pub fn team_open_filter(team_id: &str, filter_terms: &[FilterTerm]) -> Value {
    let base = json!({
        "team": { "id": { "eq": team_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    });
    merge_filter_terms(base, filter_terms)
}

/// Merges `filter_terms` into `base` one at a time via [`merge_json_object`], each term
/// first translated to its `IssueFilter` JSON fragment by [`filter_term_fragment`]. An
/// empty slice returns `base` completely unchanged — the guarantee
/// [`assignee_open_filter`]/[`project_open_filter`]/[`team_open_filter`] each document,
/// and what keeps every existing caller (none of which have a query yet) byte-identical
/// to pre-TF-616 behavior.
///
/// Two terms of the same kind combine only when their fragments touch *different* leaf
/// keys under a shared object — e.g. `priority:>=2` and `priority:<=4` fold into one
/// `{"gte":2,"lte":4}` range, since `>=`/`<=` map to distinct comparator keys (see
/// [`filter_term_fragment`]). When two terms land on the *same* leaf key instead (two
/// `priority:>=` terms, two `state:` terms, two `label:` terms), the later term wins and
/// the earlier one is silently dropped — ordinary last-write-wins map-insert semantics.
/// `query.rs`'s `ParsedQuery::filters` doc left this an open question for TF-616 to
/// settle; this is the answer, and it's deliberately simple: no attempt is made here to
/// combine same-key repeats into an `and`/`or` group. TF-617 settled where the fix
/// belongs: `query.rs`'s `push_filter_term`/`filter_terms_collide` now dedupe exactly
/// these same-leaf-key repeats *before* they ever reach `parse_query`'s output, so a real
/// `/`-filter or `default_query` never hands this function a colliding pair anymore —
/// keeping this last-write-wins fallback consistent with `matches_filter_term`'s
/// client-side AND-all (which would otherwise disagree with "last one wins" the moment a
/// query produced two colliding terms). This function's own last-write-wins behavior is
/// kept as-is regardless, since it's still reachable by any caller — including this
/// module's own tests below — that builds a `FilterTerm` slice directly rather than
/// through `parse_query`.
fn merge_filter_terms(mut base: Value, filter_terms: &[FilterTerm]) -> Value {
    for term in filter_terms {
        merge_json_object(&mut base, filter_term_fragment(term));
    }
    base
}

/// Translates a single [`FilterTerm`] into the `IssueFilter` JSON fragment it
/// contributes, expressed the way Linear's API wants a filter comparator
/// (`NumberComparator` for `priority`, `StringComparator` for `state.name`/
/// `labels.name`): `priority:` maps its [`PriorityOp`] onto Linear's own `eq`/`gte`/
/// `lte` keys operating on [`crate::plugin::query::Priority`]'s raw `0..=4` scale (see
/// that module's "Priority ordering" doc for why no translation is needed); `state:`/
/// `label:` match by name via `eqIgnoreCase` rather than plain `eq` — the DSL's own
/// grammar is lowercase-first (`priority:high`, `sort:priority`), so a user typing
/// `state:done` expects it to match Linear's `"Done"`, not silently match nothing. The
/// comparison still runs entirely server-side — only the case-folding is delegated to
/// Linear's API rather than done here.
fn filter_term_fragment(term: &FilterTerm) -> Value {
    match term {
        FilterTerm::Priority { op, value } => {
            let comparator = match op {
                PriorityOp::Eq => "eq",
                PriorityOp::Ge => "gte",
                PriorityOp::Le => "lte",
            };
            json!({ "priority": { (comparator): value.value() } })
        }
        FilterTerm::State(name) => json!({ "state": { "name": { "eqIgnoreCase": name } } }),
        FilterTerm::Label(name) => json!({ "labels": { "name": { "eqIgnoreCase": name } } }),
    }
}

/// Deep-merges `patch`'s object fields into `target` in place. Where both hold an
/// object at the same key, their fields merge recursively rather than one replacing the
/// other — e.g. a `state:` filter term's `{"name": {"eq": ...}}` fragment combines with
/// the base filter's existing `{"type": {"nin": [...]}}` under the shared `"state"` key
/// instead of clobbering it. Any other collision (scalar, array, or an object meeting a
/// non-object) has `patch`'s value win, same as a plain map insert would. `patch` is
/// expected to always be a JSON object (every [`filter_term_fragment`] result is); a
/// non-object `patch` is a no-op rather than a panic, since there's no sensible way to
/// "merge" a scalar into an object in place.
fn merge_json_object(target: &mut Value, patch: Value) {
    let Value::Object(patch_map) = patch else {
        return;
    };
    let Value::Object(target_map) = target else {
        return;
    };

    for (key, patch_value) in patch_map {
        match target_map.get_mut(&key) {
            Some(existing) if existing.is_object() && patch_value.is_object() => {
                merge_json_object(existing, patch_value);
            }
            _ => {
                target_map.insert(key, patch_value);
            }
        }
    }
}

/// Fetch every issue matching `filter`, following `pageInfo.hasNextPage` past a single
/// `get_issues` page (a single page previously silently truncated an active project's
/// backlog at 50 issues — see `MAX_PAGES` for the fetch's own upper bound). `warn_context`
/// is folded into the truncation warning so callers stay distinguishable in logs.
async fn fetch_issues_paginated(
    client: &LinearClient,
    filter: &Value,
    warn_context: &str,
) -> Result<Vec<Issue>> {
    let mut issues = Vec::new();
    let mut after: Option<String> = None;
    let mut page = 0u32;

    loop {
        page += 1;
        let connection = client
            .get_issues(Some(filter.clone()), Some(ISSUE_PAGE_SIZE), after.take())
            .await?;
        let has_next_page = connection.page_info.has_next_page;
        after = connection.page_info.end_cursor.clone();
        issues.extend(connection.nodes);

        if !has_next_page || after.is_none() {
            break;
        }
        if page >= MAX_PAGES {
            tracing::warn!(
                "{warn_context}: still more open issues after {page} pages of \
                 {ISSUE_PAGE_SIZE} — showing the first {} only.",
                issues.len()
            );
            break;
        }
    }

    Ok(issues)
}

/// Fetch the open (not completed, not canceled) issues assigned to the currently
/// authenticated user, paginating past a single page the same way
/// [`fetch_project_issues`] does (see `fetch_issues_paginated`) so a user with more
/// than one page of open issues doesn't see a silently truncated list.
///
/// `LinearClient` has no dedicated "my issues" call, so this composes
/// `get_viewer()` (to find the current user id) with `get_issues()` filtered
/// to that id as assignee, excluding terminal-state issues so completed/canceled
/// work doesn't clutter the daily list. `get_viewer()` is already covered by
/// `LinearClient`'s own tests; this function is thin composition on top.
///
/// `filter_terms` (TF-615's parsed `priority:`/`state:`/`label:` terms) is threaded
/// straight through to [`assignee_open_filter`] — pass `&[]` for today's unfiltered
/// behavior; TF-617 wires a real slice in from the active query.
pub async fn fetch_my_issues(
    client: &LinearClient,
    filter_terms: &[FilterTerm],
) -> Result<Vec<Issue>> {
    let viewer = client.get_viewer().await?;
    let filter = assignee_open_filter(&viewer.id, filter_terms);
    fetch_issues_paginated(client, &filter, "Your assigned issues").await
}

/// Fetch every open issue of `project_id`, following `pageInfo.hasNextPage` past a single
/// `get_issues` page via `fetch_issues_paginated` (a single page previously silently
/// truncated an active project's backlog at 50 issues — see `MAX_PAGES` for the fetch's
/// own upper bound).
///
/// `filter_terms` is threaded straight through to [`project_open_filter`] — see
/// [`fetch_my_issues`] for the `&[]`/TF-617 note.
pub async fn fetch_project_issues(
    client: &LinearClient,
    project_id: &str,
    filter_terms: &[FilterTerm],
) -> Result<Vec<Issue>> {
    let filter = project_open_filter(project_id, filter_terms);
    fetch_issues_paginated(client, &filter, &format!("Project {project_id}")).await
}

/// Fetch every project in the workspace, following `pageInfo.hasNextPage` past a single
/// `get_projects` page. Needed because [`fetch_current_project_issues`] must search the
/// whole workspace by name — a single 250-project page previously made a project past
/// that point silently unmatchable, surfacing as a misleading "no project matches" error
/// instead of a pagination gap (see `MAX_PAGES` for the fetch's own upper bound).
async fn fetch_all_projects(client: &LinearClient) -> Result<Vec<Project>> {
    let mut projects = Vec::new();
    let mut after: Option<String> = None;
    let mut page = 0u32;

    loop {
        page += 1;
        let connection = client
            .get_projects(None, Some(PROJECT_PAGE_SIZE), after.take())
            .await?;
        let has_next_page = connection.page_info.has_next_page;
        after = connection.page_info.end_cursor.clone();
        projects.extend(connection.nodes);

        if !has_next_page || after.is_none() {
            break;
        }
        if page >= MAX_PAGES {
            tracing::warn!(
                "Workspace still has more projects after {page} pages of {PROJECT_PAGE_SIZE} \
                 — project name matching may miss projects beyond the {} fetched. Add a \
                 `[project_overrides]` entry for this repo to config.toml to bypass name \
                 matching.",
                projects.len()
            );
            break;
        }
    }

    Ok(projects)
}

/// Resolve the Linear project matching the current working directory, then fetch its
/// open issues.
///
/// `repo::detect_repo_name` (CWD/git remote) runs first and unconditionally — it's the
/// lookup key for a repo-scoped `[project_overrides]` entry (see
/// `config::load_project_id_override`), so unlike the override this replaces, it can no
/// longer be skipped even when an override is configured. A configured override for this
/// specific repo still short-circuits the more expensive half: `fetch_all_projects`'s
/// network call (paginated `get_projects`) and `repo::match_project`'s name matching are
/// both skipped in that case — and, just as importantly, means a workspace-wide project
/// fetch failing (bad scope, timeout) can't break a view whose project id was already
/// known from config.
///
/// Without an override for this repo, this composes `fetch_all_projects` and
/// `repo::match_project` (name matching) to find the project id, then delegates to
/// `fetch_project_issues`. Re-runs every step on each call — no caching — so a
/// `config.toml` edit or a `git remote` change between calls (e.g. across a retry) is
/// picked up rather than served stale.
///
/// `filter_terms` is threaded straight through to [`fetch_project_issues`] — see
/// [`fetch_my_issues`] for the `&[]`/TF-617 note.
pub async fn fetch_current_project_issues(
    client: &LinearClient,
    filter_terms: &[FilterTerm],
) -> Result<Vec<Issue>> {
    let repo_name = repo::detect_repo_name();
    let project_id_override = config::load_project_id_override(&repo_name)?;

    let project_id = match project_id_override {
        Some(id) => id,
        None => {
            let projects = fetch_all_projects(client).await?;
            let config_path_hint = config::current_config_path_hint();
            repo::match_project(&repo_name, &projects, &config_path_hint)?
                .id
                .clone()
        }
    };

    fetch_project_issues(client, &project_id, filter_terms).await
}

/// Fetch every team in the workspace, following `pageInfo.hasNextPage` past a single
/// `get_teams` page the same way [`fetch_all_projects`] does for projects (see
/// `MAX_PAGES` for the fetch's own upper bound). Needed because [`resolve_team_id`]
/// must see every team to tell "exactly one" from "more than one".
async fn fetch_all_teams(client: &LinearClient) -> Result<Vec<Team>> {
    let mut teams = Vec::new();
    let mut after: Option<String> = None;
    let mut page = 0u32;

    loop {
        page += 1;
        let connection = client.get_teams(Some(TEAM_PAGE_SIZE), after.take()).await?;
        let has_next_page = connection.page_info.has_next_page;
        after = connection.page_info.end_cursor.clone();
        teams.extend(connection.nodes);

        if !has_next_page || after.is_none() {
            break;
        }
        if page >= MAX_PAGES {
            tracing::warn!(
                "Workspace still has more teams after {page} pages of {TEAM_PAGE_SIZE} \
                 — team selection may miss teams beyond the {} fetched. Set `team_id` \
                 in config.toml to bypass team listing.",
                teams.len()
            );
            break;
        }
    }

    Ok(teams)
}

/// Decide which of `teams` [`fetch_current_team_issues`] should show, given no
/// configured `team_id` override: exactly one team resolves to it automatically
/// (nothing to disambiguate); zero or more than one is an `Error::ConfigError` naming
/// the candidates (if any) and telling the user to set `team_id` in
/// `config_path_hint`. Unlike the project override, there's no per-repo signal (name,
/// git remote) to match a team by — Linear teams aren't tied to a single repo the way
/// projects are — so this is purely a count-based decision.
///
/// Split out from [`resolve_team_id`] as a pure, synchronous function — the same way
/// [`crate::plugin::repo::match_project`] is split from its own async caller — so the
/// disambiguation logic (and its error-message formatting) can be unit tested without
/// a mock `LinearClient`.
fn pick_team<'a>(teams: &'a [Team], config_path_hint: &str) -> Result<&'a Team> {
    match teams.len() {
        1 => Ok(&teams[0]),
        0 => Err(Error::ConfigError(format!(
            "No teams found in this Linear workspace. Set `team_id` in \
             {config_path_hint} once you know which team to use."
        ))),
        _ => {
            let names = teams
                .iter()
                .map(|team| format!("{} ({})", team.name, team.key))
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::ConfigError(format!(
                "Multiple teams found: {names}. Set `team_id` in {config_path_hint} to \
                 pick one."
            )))
        }
    }
}

/// Resolve which team's issues [`fetch_current_team_issues`] should show.
///
/// A configured `team_id` (see `config::load_team_id_override`) short-circuits this,
/// skipping [`fetch_all_teams`]'s network call entirely — mirrors how
/// `fetch_current_project_issues`'s `project_overrides` short-circuits
/// `fetch_all_projects`. Without one, every team in the workspace is fetched and
/// handed to [`pick_team`] to decide.
async fn resolve_team_id(client: &LinearClient) -> Result<String> {
    if let Some(team_id) = config::load_team_id_override()? {
        return Ok(team_id);
    }

    let teams = fetch_all_teams(client).await?;
    let config_path_hint = config::current_config_path_hint();

    Ok(pick_team(&teams, &config_path_hint)?.id.clone())
}

/// Fetch every open (not completed, not canceled) issue of `team_id`, following
/// `pageInfo.hasNextPage` past a single `get_issues` page via
/// `fetch_issues_paginated`, the same way [`fetch_project_issues`] composes
/// [`project_open_filter`].
///
/// Composes `client.get_issues` with [`team_open_filter`] rather than delegating to
/// [`LinearClient::get_team_issues`] — TF-579's suggested entry point, and still a
/// valid public API in its own right — because that method only filters on `team.id`
/// (not open state) and takes no pagination cursor. Delegating to it would mean
/// filtering "open" out of the first `ISSUE_PAGE_SIZE` issues of *any* state, not the
/// first `ISSUE_PAGE_SIZE` *open* ones: any team with more than `ISSUE_PAGE_SIZE`
/// issues total (open + closed, not just open) would risk silently dropping open
/// ones from the view — and since `QUERY_ISSUES` has no `orderBy` and Linear's
/// default order isn't "open first", a team with a large closed backlog could show
/// zero open issues while several exist, with no truncation warning at all (unlike
/// every other pagination loop in this file). Filtering at the query and paginating
/// via `fetch_issues_paginated` avoids all of that — the same way TF-577/578 fixed
/// the equivalent gap for projects/my-issues.
///
/// `filter_terms` is threaded straight through to [`team_open_filter`] — see
/// [`fetch_my_issues`] for the `&[]`/TF-617 note.
pub async fn fetch_team_issues(
    client: &LinearClient,
    team_id: &str,
    filter_terms: &[FilterTerm],
) -> Result<Vec<Issue>> {
    let filter = team_open_filter(team_id, filter_terms);
    fetch_issues_paginated(client, &filter, &format!("Team {team_id}")).await
}

/// Resolve which team to show (see `resolve_team_id`), then fetch its open issues.
/// Re-runs resolution on every call — no caching — matching
/// [`fetch_current_project_issues`], so a `config.toml` edit between calls (e.g.
/// across a retry) is picked up rather than served stale.
///
/// `filter_terms` is threaded straight through to [`fetch_team_issues`] — see
/// [`fetch_my_issues`] for the `&[]`/TF-617 note.
pub async fn fetch_current_team_issues(
    client: &LinearClient,
    filter_terms: &[FilterTerm],
) -> Result<Vec<Issue>> {
    let team_id = resolve_team_id(client).await?;
    fetch_team_issues(client, &team_id, filter_terms).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::query::Priority;

    #[test]
    fn assignee_open_filter_matches_assignee_and_excludes_terminal_states() {
        let filter = assignee_open_filter("user-123", &[]);

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }

    #[test]
    fn project_open_filter_matches_project_and_excludes_terminal_states() {
        let filter = project_open_filter("project-123", &[]);

        assert_eq!(filter["project"]["id"]["eq"], "project-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }

    #[test]
    fn team_open_filter_matches_team_and_excludes_terminal_states() {
        let filter = team_open_filter("team-123", &[]);

        assert_eq!(filter["team"]["id"]["eq"], "team-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }

    #[test]
    fn open_filter_with_no_filter_terms_reproduces_exact_pre_tf_616_json() {
        // Empty `filter_terms` must be a true no-op: byte-identical to what these
        // functions returned before TF-616 added the parameter, since every existing
        // caller (none of which have a query yet) passes `&[]`.
        assert_eq!(
            assignee_open_filter("user-123", &[]),
            json!({
                "assignee": { "id": { "eq": "user-123" } },
                "state": { "type": { "nin": ["completed", "canceled"] } }
            })
        );
        assert_eq!(
            project_open_filter("project-123", &[]),
            json!({
                "project": { "id": { "eq": "project-123" } },
                "state": { "type": { "nin": ["completed", "canceled"] } }
            })
        );
        assert_eq!(
            team_open_filter("team-123", &[]),
            json!({
                "team": { "id": { "eq": "team-123" } },
                "state": { "type": { "nin": ["completed", "canceled"] } }
            })
        );
    }

    #[test]
    fn assignee_open_filter_merges_a_single_priority_term() {
        let term = FilterTerm::Priority {
            op: PriorityOp::Ge,
            value: Priority::new(2).unwrap(),
        };
        let filter = assignee_open_filter("user-123", &[term]);

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
        assert_eq!(filter["priority"]["gte"], 2);
    }

    #[test]
    fn assignee_open_filter_priority_eq_and_le_map_to_eq_and_lte() {
        let eq_term = FilterTerm::Priority {
            op: PriorityOp::Eq,
            value: Priority::new(1).unwrap(),
        };
        assert_eq!(
            assignee_open_filter("user-123", &[eq_term])["priority"]["eq"],
            1
        );

        let le_term = FilterTerm::Priority {
            op: PriorityOp::Le,
            value: Priority::new(3).unwrap(),
        };
        assert_eq!(
            assignee_open_filter("user-123", &[le_term])["priority"]["lte"],
            3
        );
    }

    #[test]
    fn assignee_open_filter_merges_two_priority_terms_with_different_comparators_into_one_range() {
        // A query like `priority:>=2 priority:<=3` produces two FilterTerm::Priority
        // entries — repeated terms of the same kind aren't deduped upstream (see
        // ParsedQuery::filters' doc in query.rs) — that must combine into a single
        // range fragment rather than one overwriting the other, since `>=`/`<=` map to
        // distinct comparator keys under the shared "priority" object.
        let terms = [
            FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: Priority::new(2).unwrap(),
            },
            FilterTerm::Priority {
                op: PriorityOp::Le,
                value: Priority::new(3).unwrap(),
            },
        ];
        let filter = assignee_open_filter("user-123", &terms);

        assert_eq!(filter["priority"]["gte"], 2);
        assert_eq!(filter["priority"]["lte"], 3);
    }

    #[test]
    fn assignee_open_filter_second_of_two_same_comparator_priority_terms_silently_wins() {
        // Unlike the different-comparator case above, two `priority:>=` terms land on
        // the *same* leaf key ("gte") — documents the deliberate last-write-wins
        // collision behavior described in merge_filter_terms' doc.
        let terms = [
            FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: Priority::new(2).unwrap(),
            },
            FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: Priority::new(4).unwrap(),
            },
        ];
        let filter = assignee_open_filter("user-123", &terms);

        assert_eq!(filter["priority"]["gte"], 4);
    }

    #[test]
    fn project_open_filter_merges_a_single_state_term_alongside_the_base_state_filter() {
        // The base filter already has a `"state"` key (`type: { nin: [...] }`); a
        // `state:` term must merge its `name` constraint into that same object rather
        // than replacing it.
        let filter = project_open_filter("project-123", &[FilterTerm::State("In Review".into())]);

        assert_eq!(filter["project"]["id"]["eq"], "project-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
        assert_eq!(filter["state"]["name"]["eqIgnoreCase"], "In Review");
    }

    #[test]
    fn open_filter_second_of_two_colliding_state_terms_silently_wins() {
        // Two `state:` terms collide on the same leaf key ("state.name.eqIgnoreCase").
        // Documents the current last-write-wins behavior of merge_json_object's
        // non-object-leaf collision branch.
        let filter = project_open_filter(
            "project-123",
            &[
                FilterTerm::State("Todo".into()),
                FilterTerm::State("Done".into()),
            ],
        );

        assert_eq!(filter["state"]["name"]["eqIgnoreCase"], "Done");
        // The base filter's own "state.type" constraint must survive the collision on
        // the sibling "state.name" key — it's a different leaf under the same object.
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }

    #[test]
    fn team_open_filter_merges_a_single_label_term() {
        let filter = team_open_filter("team-123", &[FilterTerm::Label("urgent-fix".into())]);

        assert_eq!(filter["team"]["id"]["eq"], "team-123");
        assert_eq!(filter["labels"]["name"]["eqIgnoreCase"], "urgent-fix");
    }

    #[test]
    fn open_filter_second_of_two_colliding_label_terms_silently_wins() {
        let filter = team_open_filter(
            "team-123",
            &[
                FilterTerm::Label("bug".into()),
                FilterTerm::Label("urgent".into()),
            ],
        );

        assert_eq!(filter["labels"]["name"]["eqIgnoreCase"], "urgent");
    }

    #[test]
    fn open_filter_merges_multiple_filter_terms_together_with_the_base_filter() {
        let terms = [
            FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: Priority::new(2).unwrap(),
            },
            FilterTerm::State("In Review".into()),
            FilterTerm::Label("urgent-fix".into()),
        ];
        let filter = assignee_open_filter("user-123", &terms);

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
        assert_eq!(filter["state"]["name"]["eqIgnoreCase"], "In Review");
        assert_eq!(filter["priority"]["gte"], 2);
        assert_eq!(filter["labels"]["name"]["eqIgnoreCase"], "urgent-fix");
    }

    #[test]
    fn merge_json_object_non_object_patch_is_a_no_op() {
        let mut target = json!({ "a": 1 });
        merge_json_object(&mut target, json!("not an object"));
        assert_eq!(target, json!({ "a": 1 }));
    }

    #[test]
    fn merge_json_object_non_object_target_is_a_no_op() {
        let mut target = json!("scalar target");
        merge_json_object(&mut target, json!({ "a": 1 }));
        assert_eq!(target, json!("scalar target"));
    }

    #[test]
    fn merge_json_object_patch_object_overwrites_a_non_object_existing_value_at_the_same_key() {
        // existing["priority"] is a scalar, patch["priority"] is an object — the
        // `existing.is_object() && patch_value.is_object()` guard fails, so this falls
        // to the `_` arm and the object wins outright rather than attempting a partial
        // merge into a non-object.
        let mut target = json!({ "priority": 5 });
        merge_json_object(&mut target, json!({ "priority": { "gte": 2 } }));
        assert_eq!(target, json!({ "priority": { "gte": 2 } }));
    }

    #[test]
    fn merge_json_object_recursively_merges_disjoint_keys_under_a_shared_object_key() {
        let mut target = json!({ "state": { "type": { "nin": ["completed"] } } });
        merge_json_object(
            &mut target,
            json!({ "state": { "name": { "eqIgnoreCase": "Done" } } }),
        );
        assert_eq!(
            target,
            json!({
                "state": {
                    "type": { "nin": ["completed"] },
                    "name": { "eqIgnoreCase": "Done" }
                }
            })
        );
    }

    fn sample_team(id: &str, key: &str, name: &str) -> Team {
        Team {
            id: id.to_string(),
            key: key.to_string(),
            name: name.to_string(),
            description: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn pick_team_resolves_automatically_when_exactly_one_team() {
        let teams = vec![sample_team("team-1", "ENG", "Engineering")];

        let team = pick_team(&teams, "/config/config.toml").unwrap();

        assert_eq!(team.id, "team-1");
    }

    #[test]
    fn pick_team_errors_naming_the_config_path_when_no_teams() {
        let err = pick_team(&[], "/config/config.toml").unwrap_err();

        let message = err.to_string();
        assert!(message.contains("No teams found"));
        assert!(message.contains("/config/config.toml"));
    }

    #[test]
    fn pick_team_errors_and_names_every_candidate_when_multiple_teams() {
        let teams = vec![
            sample_team("team-1", "ENG", "Engineering"),
            sample_team("team-2", "DES", "Design"),
        ];

        let err = pick_team(&teams, "/config/config.toml").unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Engineering (ENG)"));
        assert!(message.contains("Design (DES)"));
        assert!(message.contains("/config/config.toml"));
    }
}
