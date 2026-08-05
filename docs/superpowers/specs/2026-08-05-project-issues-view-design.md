# "Project Issues" view — design

**Linear issue**: [TF-578](https://linear.app/talent-factory/issue/TF-578) — "Project Issues"-View: alle
offenen Issues des erkannten Projekts
**Depends on**: [TF-576](https://linear.app/talent-factory/issue/TF-576) (view switcher, merged),
[TF-577](https://linear.app/talent-factory/issue/TF-577) (CWD → Linear project detection, merged)

## Problem

The plugin's view-selection menu (TF-576) already lists "Project Issues" as an entry, and
CWD → Linear-project resolution (TF-577) already exists as `repo::detect_repo_name` +
`repo::resolve_project_id` — but nothing calls them yet, so the menu entry is `available:
false` and selecting it is impossible. This task wires the two together: fetch all open
issues of the Linear project matching the current working directory, and let the menu
entry into it.

## Scope

Purely additive. `ViewKind::ProjectIssues` and its `MENU_OPTIONS` entry already exist
(TF-576); `Screen`/`ViewState`/`handle_key` in `app.rs` and all of `ui.rs` are already
generic over `ViewKind` (they render/navigate whatever view is current, regardless of
kind) — none of that changes. The work is: one new filter + two new fetch functions in
`data.rs`, flipping one `bool` in `app.rs`, and one new match arm in `main.rs`.

## Architecture

### `src/plugin/data.rs` (extended)

- `project_open_filter(project_id: &str) -> Value` — pure filter builder, same shape as
  the existing `assignee_filter`: `{ "project": { "id": { "eq": project_id } }, "state":
  { "type": { "nin": ["completed", "canceled"] } } }`. "Open" is defined as "not completed
  and not canceled" (i.e. `backlog`/`unstarted`/`started` per `IssueState::type`), matching
  the ticket's "alle offenen Issues" wording without hard-coding the open-state names as an
  allowlist that would need updating if Linear adds a new non-terminal state type.
- `fetch_project_issues(client: &LinearClient, project_id: &str) -> Result<Vec<Issue>>` —
  the ticket's named function. `client.get_issues(Some(project_open_filter(project_id)),
  Some(50), None)`, returning `connection.nodes`. Directly analogous to
  `client.rs::get_team_issues`, but as a `data.rs` composer (so the open-state filter lives
  next to the other view-specific filters) rather than a `LinearClient` method.
- `fetch_current_project_issues(client: &LinearClient) -> Result<Vec<Issue>>` — the
  CWD-resolution composer, kept in `data.rs` alongside `fetch_my_issues` so `data.rs` stays
  the single place owning every Linear data composition the plugin needs (`repo.rs` and
  `config.rs` stay network-free, matching their existing scope). Steps:
  1. `repo::detect_repo_name()` — impure, reads `git remote`/cwd.
  2. `config::load_project_id_override()` — impure, reads `config.toml`.
  3. `client.get_projects(None, Some(250))` — network.
  4. `repo::resolve_project_id(override.as_deref(), &repo_name, &projects.nodes)` — pure,
     returns the resolved `project_id` or an `Error::ConfigError` (no match / ambiguous).
  5. `fetch_project_issues(client, &project_id)`.

  No caching: every call re-runs all five steps from scratch, matching `fetch_my_issues`
  (which also re-resolves the viewer on every call) and sidestepping stale-cache bugs if
  `config.toml`'s override or the repo's git remote changes between attempts.

### `src/plugin/app.rs` (one-line change)

- `MENU_OPTIONS`'s `ProjectIssues` entry: `available: false` → `available: true`.

### `src/main.rs` (one new match arm)

- `load_issues`: add `Some(ViewKind::ProjectIssues) => match
  data::fetch_current_project_issues(client).await { Ok(issues) => app.set_issues(issues),
  Err(err) => app.set_error(err.to_string()) }`, identical shape to the existing
  `MyIssues` arm.

## Error handling

No new error variants. Every failure step — `get_projects` network/GraphQL errors,
`resolve_project_id`'s existing "no match"/"ambiguous" `Error::ConfigError` (which already
tells the user to set `project_id` in `config.toml`) — flows through the same `Result ->
.to_string() -> app.set_error()` path `MyIssues` uses today. `r` (retry) re-runs
`fetch_current_project_issues` from scratch, so it also re-resolves the project, not just
re-fetches issues for a previously-resolved id.

## Testing strategy

- `project_open_filter`: new unit test mirroring
  `assignee_filter_matches_on_the_given_user_id`, asserting the JSON shape (project id eq,
  state type nin `["completed", "canceled"]`).
- `app.rs`: new test asserting entering `ProjectIssues` from the menu transitions to
  `Screen::View(ViewKind::ProjectIssues, ViewState::Loading)` and returns
  `Action::EnterView`, mirroring the existing `MyIssues` version.
  **Fix required**: `entering_an_unavailable_option_does_nothing` and
  `enter_key_on_an_unavailable_menu_option_does_nothing` currently target `ProjectIssues`
  as their "unavailable" case (one `move_menu_selection_down` / one `Down` key) — both move
  to target `TeamIssues` instead (two steps down), which stays unavailable.
- `fetch_project_issues` / `fetch_current_project_issues` stay untested at the
  async-composition level, consistent with `fetch_my_issues` today (no `mockito` coverage
  there either) — the underlying `get_issues`/`get_projects` calls already have their own
  `client.rs` tests, and `resolve_project_id`'s resolution logic already has its own
  `repo.rs` tests.

## Out of scope

- Caching the resolved project id across retries or plugin runs.
- A dedicated "which project did we resolve to?" indicator in the UI — the view's block
  title still just reads `kind.label()` ("Project Issues"), unchanged from TF-576.
- Team Issues (TF-579) — its `MENU_OPTIONS` entry stays `available: false`.
