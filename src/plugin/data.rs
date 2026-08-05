//! Composes existing `LinearClient` calls into what the plugin needs: the
//! authenticated viewer's assigned issues.

use crate::{Issue, LinearClient, Result};
use serde_json::{json, Value};

/// A Linear issue filter matching issues assigned to `user_id`.
pub fn assignee_filter(user_id: &str) -> Value {
    json!({ "assignee": { "id": { "eq": user_id } } })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_filter_matches_on_the_given_user_id() {
        let filter = assignee_filter("user-123");

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
    }
}
