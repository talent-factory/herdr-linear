//! Composes existing `LinearClient` calls into what the plugin needs: the
//! authenticated viewer's assigned issues, and the current project's open issues.

use crate::plugin::{config, repo};
use crate::{Issue, LinearClient, Result};
use serde_json::{json, Value};

/// A Linear issue filter matching issues assigned to `user_id`.
pub fn assignee_filter(user_id: &str) -> Value {
    json!({ "assignee": { "id": { "eq": user_id } } })
}

/// A Linear issue filter matching open (not completed, not canceled) issues in
/// `project_id`. "Open" is expressed as an exclusion (`nin`) rather than an allowlist
/// of the non-terminal state types (`backlog`/`unstarted`/`started`), so it stays
/// correct if Linear ever adds another non-terminal state type.
pub fn project_open_filter(project_id: &str) -> Value {
    json!({
        "project": { "id": { "eq": project_id } },
        "state": { "type": { "nin": ["completed", "canceled"] } }
    })
}

/// Fetch the issues assigned to the currently authenticated user.
///
/// `LinearClient` has no dedicated "my issues" call, so this composes
/// `get_viewer()` (to find the current user id) with `get_issues()` filtered
/// to that id as assignee. Both underlying calls are already covered by
/// `LinearClient`'s own tests; this function is thin composition on top.
pub async fn fetch_my_issues(client: &LinearClient) -> Result<Vec<Issue>> {
    let viewer = client.get_viewer().await?;
    let connection = client
        .get_issues(Some(assignee_filter(&viewer.id)), Some(50), None)
        .await?;
    Ok(connection.nodes)
}

/// Fetch the open issues of `project_id`.
pub async fn fetch_project_issues(client: &LinearClient, project_id: &str) -> Result<Vec<Issue>> {
    let connection = client
        .get_issues(Some(project_open_filter(project_id)), Some(50), None)
        .await?;
    Ok(connection.nodes)
}

/// Resolve the Linear project matching the current working directory, then fetch its
/// open issues.
///
/// Composes `repo::detect_repo_name` (CWD/git remote), `config::load_project_id_override`
/// (config.toml), `client.get_projects` (network), and `repo::resolve_project_id` (name
/// matching or override short-circuit) to find the project id, then delegates to
/// `fetch_project_issues`. Re-runs every step on each call — no caching — so a
/// `config.toml` edit or a `git remote` change between calls (e.g. across a retry) is
/// picked up rather than served stale.
pub async fn fetch_current_project_issues(client: &LinearClient) -> Result<Vec<Issue>> {
    let repo_name = repo::detect_repo_name();
    let project_id_override = config::load_project_id_override()?;
    let projects = client.get_projects(None, Some(250)).await?;
    let project_id =
        repo::resolve_project_id(project_id_override.as_deref(), &repo_name, &projects.nodes)?;
    fetch_project_issues(client, &project_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_filter_matches_on_the_given_user_id() {
        let filter = assignee_filter("user-123");

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
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
