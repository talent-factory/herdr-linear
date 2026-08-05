//! Data models representing Linear API types

use serde::{Deserialize, Serialize};

/// Represents a Linear user
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Represents a Linear team
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Represents a Linear issue
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: String,
    pub identifier: String, // e.g., "ENG-123"
    pub title: String,
    pub description: Option<String>,
    pub state: IssueState,
    pub priority: i32,
    pub estimate: Option<i32>,
    pub team: Team,
    pub assignee: Option<User>,
    pub creator: Option<User>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub cycle: Option<Cycle>,
    pub project: Option<Project>,
    pub labels: LabelConnection,
    pub url: String,
}

/// Represents an issue workflow state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueState {
    pub id: String,
    pub name: String,
    pub r#type: String, // "backlog", "unstarted", "started", "completed", "canceled"
}

/// Represents a Linear project
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub url: String,
    pub lead_id: Option<String>,
    pub lead: Option<User>,
    pub status: ProjectStatus,
    pub created_at: String,
    pub updated_at: String,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
}

/// Represents a Linear project's status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub id: String,
    pub name: String,
    pub r#type: String, // "planned", "started", "completed", "canceled"
}

/// Represents a Linear cycle (sprint)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cycle {
    pub id: String,
    pub number: i32,
    pub name: Option<String>,
    pub team: Team,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Represents a Linear label
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Wraps an issue's `labels(first: N) { nodes { ... } }` selection, which the
/// Linear API returns as a connection object rather than a bare array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelConnection {
    pub nodes: Vec<Label>,
}

/// Represents a Linear comment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub body: String,
    pub user: User,
    pub created_at: String,
    pub updated_at: String,
}

/// Pagination cursor info for Linear API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

/// Generic connection response for paginated results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection<T> {
    pub nodes: Vec<T>,
    pub page_info: PageInfo,
}

/// GraphQL error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLError {
    pub message: String,
    pub locations: Option<Vec<GraphQLLocation>>,
    pub path: Option<Vec<serde_json::Value>>,
    pub extensions: Option<serde_json::Value>,
}

/// GraphQL error location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLLocation {
    pub line: i32,
    pub column: i32,
}

/// Generic GraphQL response wrapper
#[derive(Debug, Deserialize)]
pub struct GraphQLResponse<T> {
    pub data: Option<T>,
    pub errors: Option<Vec<GraphQLError>>,
}

/// Workspace (organization) in Linear
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub url_key: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_user() -> serde_json::Value {
        json!({
            "id": "user-1",
            "email": "alice@example.com",
            "name": "Alice",
            "avatarUrl": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        })
    }

    fn sample_team() -> serde_json::Value {
        json!({
            "id": "team-1",
            "key": "ENG",
            "name": "Engineering",
            "description": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        })
    }

    fn sample_issue() -> serde_json::Value {
        json!({
            "id": "issue-1",
            "identifier": "ENG-123",
            "title": "Fix bug",
            "description": null,
            "state": {"id": "state-1", "name": "In Progress", "type": "started"},
            "priority": 2,
            "estimate": null,
            "team": sample_team(),
            "assignee": null,
            "creator": sample_user(),
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "startedAt": null,
            "completedAt": null,
            "cycle": null,
            "project": null,
            "labels": {"nodes": []},
            "url": "https://linear.app/team/issue/ENG-123"
        })
    }

    #[test]
    fn deserializes_issue_with_nested_team_and_creator() {
        let issue: Issue = serde_json::from_value(sample_issue()).expect("valid issue payload");

        assert_eq!(issue.identifier, "ENG-123");
        assert_eq!(issue.team.key, "ENG");
        assert_eq!(issue.creator.unwrap().name, "Alice");
        assert_eq!(issue.state.r#type, "started");
    }

    #[test]
    fn deserializes_connection_from_linear_camel_case_payload() {
        // Regression test: Connection<T> previously lacked `rename_all = "camelCase"`,
        // so Linear's `pageInfo` keys silently failed to deserialize and every
        // paginated call (get_teams, get_issues, get_projects, get_cycles)
        // returned a generic "Failed to parse ... data" error.
        let payload = json!({
            "nodes": [sample_issue()],
            "pageInfo": {
                "hasNextPage": true,
                "hasPreviousPage": false,
                "startCursor": "cursor-1",
                "endCursor": "cursor-2"
            }
        });

        let connection: Connection<Issue> =
            serde_json::from_value(payload).expect("valid connection payload");

        assert!(connection.page_info.has_next_page);
        assert_eq!(connection.nodes[0].identifier, "ENG-123");
    }

    #[test]
    fn graphql_response_deserializes_data_and_errors() {
        let success: GraphQLResponse<serde_json::Value> =
            serde_json::from_value(json!({"data": {"ok": true}, "errors": null}))
                .expect("valid response");
        assert!(success.data.is_some());
        assert!(success.errors.is_none());

        let failure: GraphQLResponse<serde_json::Value> = serde_json::from_value(json!({
            "data": null,
            "errors": [{"message": "Team not found"}]
        }))
        .expect("valid error response");

        assert!(failure.data.is_none());
        assert_eq!(failure.errors.unwrap()[0].message, "Team not found");
    }
}
