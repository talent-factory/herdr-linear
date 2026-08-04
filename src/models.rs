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
    pub avatar_url: Option<String>,
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
    pub creator: User,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub cycle: Option<Cycle>,
    pub project: Option<Project>,
    pub labels: Vec<Label>,
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
    pub state: String, // "planned", "started", "completed", "canceled"
    pub created_at: String,
    pub updated_at: String,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
}

/// Represents a Linear cycle (sprint)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cycle {
    pub id: String,
    pub number: i32,
    pub title: String,
    pub team: Team,
    pub started_at: Option<String>,
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
pub struct Connection<T> {
    pub nodes: Vec<T>,
    pub page_info: PageInfo,
    pub total_count: i32,
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
