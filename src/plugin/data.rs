//! Composes existing `LinearClient` calls into what the plugin needs: the
//! authenticated viewer's open assigned issues, and the current project's open issues.

use crate::plugin::{config, repo};
use crate::{Issue, LinearClient, Project, Result};
use serde_json::{json, Value};

/// Page size used when paginating `get_issues`/`get_projects` to completion (see
/// [`fetch_issues_paginated`]/[`fetch_all_projects`]). 50 matches `LinearClient`'s own
/// default; Linear's API caps a single page at 250 regardless of what's requested.
const ISSUE_PAGE_SIZE: i32 = 50;
const PROJECT_PAGE_SIZE: i32 = 250;

/// Hang/cost guard on the pagination loops below: stop after this many pages even if
/// Linear still reports more, logging a warning rather than fetching indefinitely. Chosen
/// generously above any real project's open-issue count or workspace's project count —
/// hitting it means something (a filter bug, a runaway workspace) — but it's a fetch of
/// a few thousand records at worst, not an unbounded one.
const MAX_PAGES: u32 = 20;

/// A Linear issue filter matching open (not completed, not canceled) issues assigned to
/// `user_id`. "Open" is expressed as an exclusion (`nin`) rather than an allowlist of the
/// non-terminal state types (`triage`/`backlog`/`unstarted`/`started`), so it can't
/// silently drop issues in a state type this code doesn't know about — mirrors
/// [`project_open_filter`].
pub fn assignee_open_filter(user_id: &str) -> Value {
    json!({
        "assignee": { "id": { "eq": user_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    })
}

/// A Linear issue filter matching open (not completed, not canceled) issues in
/// `project_id`. "Open" is expressed as an exclusion (`nin`) rather than an allowlist
/// of the non-terminal state types (`triage`/`backlog`/`unstarted`/`started`), so it
/// can't silently drop issues in a state type this code doesn't know about — mirrors
/// [`assignee_open_filter`].
pub fn project_open_filter(project_id: &str) -> Value {
    json!({
        "project": { "id": { "eq": project_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    })
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
pub async fn fetch_my_issues(client: &LinearClient) -> Result<Vec<Issue>> {
    let viewer = client.get_viewer().await?;
    let filter = assignee_open_filter(&viewer.id);
    fetch_issues_paginated(client, &filter, "Your assigned issues").await
}

/// Fetch every open issue of `project_id`, following `pageInfo.hasNextPage` past a single
/// `get_issues` page via `fetch_issues_paginated` (a single page previously silently
/// truncated an active project's backlog at 50 issues — see `MAX_PAGES` for the fetch's
/// own upper bound).
pub async fn fetch_project_issues(client: &LinearClient, project_id: &str) -> Result<Vec<Issue>> {
    let filter = project_open_filter(project_id);
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
pub async fn fetch_current_project_issues(client: &LinearClient) -> Result<Vec<Issue>> {
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

    fetch_project_issues(client, &project_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_open_filter_matches_assignee_and_excludes_terminal_states() {
        let filter = assignee_open_filter("user-123");

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }

    #[test]
    fn project_open_filter_matches_project_and_excludes_terminal_states() {
        let filter = project_open_filter("project-123");

        assert_eq!(filter["project"]["id"]["eq"], "project-123");
        assert_eq!(
            filter["state"]["type"]["nin"],
            json!(["completed", "canceled"])
        );
    }
}
