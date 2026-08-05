//! Linear GraphQL API client

use crate::error::{api_error, graphql_error, Error, Result};
use crate::models::*;
use crate::queries::*;
use reqwest::{Client as HttpClient, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, error, warn};

const LINEAR_API_ENDPOINT: &str = "https://api.linear.app/graphql";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Linear GraphQL API client
pub struct LinearClient {
    http_client: HttpClient,
    api_key: String,
    endpoint: String,
}

impl LinearClient {
    /// Create a new Linear client with API key
    ///
    /// # Arguments
    /// * `api_key` - Linear API key (format: `lin_api_*`)
    ///
    /// # Errors
    /// Returns error if API key is invalid or empty
    pub fn new<S: Into<String>>(api_key: S) -> Result<Self> {
        let api_key = api_key.into();

        if api_key.is_empty() {
            return Err(Error::InvalidApiKey);
        }

        let http_client = HttpClient::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| Error::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            http_client,
            api_key,
            endpoint: LINEAR_API_ENDPOINT.to_string(),
        })
    }

    /// Create a client pointed at a custom endpoint (used by tests to target a mock server)
    #[cfg(test)]
    fn with_endpoint<S: Into<String>>(api_key: S, endpoint: String) -> Result<Self> {
        let mut client = Self::new(api_key)?;
        client.endpoint = endpoint;
        Ok(client)
    }

    /// Get the authenticated user (viewer)
    pub async fn get_viewer(&self) -> Result<User> {
        debug!("Fetching viewer");
        let response = self
            .query::<serde_json::Value>(QUERY_VIEWER, json!({}))
            .await?;

        response
            .get("viewer")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse viewer data"))
    }

    /// Get all teams
    pub async fn get_teams(
        &self,
        limit: Option<i32>,
        after: Option<String>,
    ) -> Result<Connection<Team>> {
        debug!("Fetching teams");
        let variables = json!({
            "first": limit.unwrap_or(50),
            "after": after
        });

        let response = self
            .query::<serde_json::Value>(QUERY_TEAMS, variables)
            .await?;

        response
            .get("teams")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse teams data"))
    }

    /// Get a team by ID
    pub async fn get_team(&self, team_id: &str) -> Result<Team> {
        debug!("Fetching team: {}", team_id);
        let variables = json!({"id": team_id});
        let response = self
            .query::<serde_json::Value>(QUERY_TEAM, variables)
            .await?;

        response
            .get("team")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .ok_or_else(|| graphql_error(format!("Team not found: {}", team_id)))
    }

    /// Get issues with optional filters
    ///
    /// # Arguments
    /// * `filter` - Optional filter criteria (JSON value)
    /// * `limit` - Max number of issues to return (default: 50)
    /// * `after` - Cursor for pagination
    pub async fn get_issues(
        &self,
        filter: Option<Value>,
        limit: Option<i32>,
        after: Option<String>,
    ) -> Result<Connection<Issue>> {
        debug!("Fetching issues with filter: {:?}", filter);
        let variables = json!({
            "first": limit.unwrap_or(50),
            "after": after,
            "filter": filter.unwrap_or(Value::Null)
        });

        let response = self
            .query::<serde_json::Value>(QUERY_ISSUES, variables)
            .await?;

        response
            .get("issues")
            .and_then(|i| serde_json::from_value(i.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse issues data"))
    }

    /// Get a single issue by ID
    pub async fn get_issue(&self, issue_id: &str) -> Result<Issue> {
        debug!("Fetching issue: {}", issue_id);
        let variables = json!({"id": issue_id});
        let response = self
            .query::<serde_json::Value>(QUERY_ISSUE, variables)
            .await?;

        response
            .get("issue")
            .and_then(|i| serde_json::from_value(i.clone()).ok())
            .ok_or_else(|| graphql_error(format!("Issue not found: {}", issue_id)))
    }

    /// Get issues for a specific team
    pub async fn get_team_issues(
        &self,
        team_id: &str,
        limit: Option<i32>,
    ) -> Result<Connection<Issue>> {
        debug!("Fetching issues for team: {}", team_id);
        let filter = json!({
            "team": {
                "id": {
                    "eq": team_id
                }
            }
        });

        self.get_issues(Some(filter), limit, None).await
    }

    /// Create a new issue
    ///
    /// # Arguments
    /// * `title` - Issue title (required)
    /// * `team_id` - Team ID (required)
    /// * `description` - Issue description (optional)
    /// * `priority` - Priority level 0-4 (optional)
    pub async fn create_issue(
        &self,
        title: &str,
        team_id: &str,
        description: Option<&str>,
        priority: Option<i32>,
    ) -> Result<Issue> {
        debug!("Creating issue: {}", title);

        let mut input = json!({
            "title": title,
            "teamId": team_id
        });

        if let Some(desc) = description {
            input["description"] = json!(desc);
        }
        if let Some(p) = priority {
            input["priority"] = json!(p);
        }

        let variables = json!({"input": input});
        let response = self
            .mutate::<serde_json::Value>(MUTATION_CREATE_ISSUE, variables)
            .await?;

        response
            .get("issueCreate")
            .and_then(|ic| ic.get("issue"))
            .and_then(|i| serde_json::from_value(i.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to create issue"))
    }

    /// Update an issue
    pub async fn update_issue(&self, issue_id: &str, updates: Value) -> Result<Issue> {
        debug!("Updating issue: {}", issue_id);

        let variables = json!({
            "id": issue_id,
            "input": updates
        });

        let response = self
            .mutate::<serde_json::Value>(MUTATION_UPDATE_ISSUE, variables)
            .await?;

        response
            .get("issueUpdate")
            .and_then(|iu| iu.get("issue"))
            .and_then(|i| serde_json::from_value(i.clone()).ok())
            .ok_or_else(|| graphql_error(format!("Failed to update issue: {}", issue_id)))
    }

    /// Add a comment to an issue
    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<Comment> {
        debug!("Adding comment to issue: {}", issue_id);

        let input = json!({
            "issueId": issue_id,
            "body": body
        });

        let variables = json!({"input": input});
        let response = self
            .mutate::<serde_json::Value>(MUTATION_ADD_COMMENT, variables)
            .await?;

        response
            .get("commentCreate")
            .and_then(|cc| cc.get("comment"))
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to add comment"))
    }

    /// Get all projects
    pub async fn get_projects(
        &self,
        filter: Option<Value>,
        limit: Option<i32>,
    ) -> Result<Connection<Project>> {
        debug!("Fetching projects");
        let variables = json!({
            "first": limit.unwrap_or(50),
            "filter": filter.unwrap_or(Value::Null)
        });

        let response = self
            .query::<serde_json::Value>(QUERY_PROJECTS, variables)
            .await?;

        response
            .get("projects")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse projects data"))
    }

    /// Get cycles for a team
    pub async fn get_cycles(&self, team_id: &str, limit: Option<i32>) -> Result<Connection<Cycle>> {
        debug!("Fetching cycles for team: {}", team_id);
        let variables = json!({
            "filter": {"team": {"id": {"eq": team_id}}},
            "first": limit.unwrap_or(50)
        });

        let response = self
            .query::<serde_json::Value>(QUERY_CYCLES, variables)
            .await?;

        response
            .get("cycles")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse cycles data"))
    }

    /// Get workflow states for a team
    pub async fn get_workflow_states(&self, team_id: &str) -> Result<Vec<IssueState>> {
        debug!("Fetching workflow states for team: {}", team_id);
        let variables = json!({"filter": {"team": {"id": {"eq": team_id}}}, "first": 100});
        let response = self
            .query::<serde_json::Value>(QUERY_WORKFLOW_STATES, variables)
            .await?;

        response
            .get("workflowStates")
            .and_then(|ws| ws.get("nodes"))
            .and_then(|nodes| serde_json::from_value(nodes.clone()).ok())
            .ok_or_else(|| graphql_error("Failed to parse workflow states"))
    }

    /// Execute a raw GraphQL query
    pub async fn query<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<T> {
        self.execute_graphql(query, variables, false).await
    }

    /// Execute a raw GraphQL mutation
    pub async fn mutate<T: serde::de::DeserializeOwned>(
        &self,
        mutation: &str,
        variables: Value,
    ) -> Result<T> {
        self.execute_graphql(mutation, variables, true).await
    }

    /// Internal method to execute GraphQL operations
    async fn execute_graphql<T: serde::de::DeserializeOwned>(
        &self,
        operation: &str,
        variables: Value,
        is_mutation: bool,
    ) -> Result<T> {
        let payload = json!({
            "query": operation,
            "variables": variables
        });

        debug!(
            "Executing {} with payload: {}",
            if is_mutation { "mutation" } else { "query" },
            payload
        );

        let response = self
            .http_client
            .post(&self.endpoint)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        let status = response.status();

        match status {
            StatusCode::OK => {
                let graphql_response: GraphQLResponse<T> = response.json().await?;

                if let Some(errors) = graphql_response.errors {
                    let error_messages = errors
                        .iter()
                        .map(|e| e.message.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    error!("GraphQL errors: {}", error_messages);
                    return Err(graphql_error(error_messages));
                }

                graphql_response
                    .data
                    .ok_or_else(|| graphql_error("No data in response"))
            }
            StatusCode::UNAUTHORIZED => {
                error!("Authentication failed - invalid API key");
                Err(Error::AuthenticationFailed(
                    "Invalid API key or insufficient permissions".to_string(),
                ))
            }
            StatusCode::TOO_MANY_REQUESTS => {
                warn!("Rate limited by Linear API");
                Err(Error::RateLimitExceeded {
                    retry_after_ms: 60000,
                })
            }
            _ => {
                let body = response.text().await.unwrap_or_default();
                error!("HTTP error {}: {}", status, body);
                Err(api_error(format!("HTTP {}: {}", status, body)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = LinearClient::new("lin_api_test_key").unwrap();
        assert_eq!(client.api_key, "lin_api_test_key");
    }

    #[test]
    fn test_invalid_api_key() {
        let result = LinearClient::new("");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_viewer_parses_successful_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "viewer": {
                            "id": "user-1",
                            "email": "alice@example.com",
                            "name": "Alice",
                            "avatarUrl": null,
                            "createdAt": "2026-01-01T00:00:00Z",
                            "updatedAt": "2026-01-01T00:00:00Z"
                        }
                    },
                    "errors": null
                })
                .to_string(),
            )
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let viewer = client.get_viewer().await.unwrap();

        assert_eq!(viewer.name, "Alice");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn graphql_errors_in_200_response_surface_as_graphql_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"data": null, "errors": [{"message": "Team not found"}]}).to_string())
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client.get_team("missing").await.unwrap_err();

        match err {
            Error::GraphQLError(msg) => assert!(msg.contains("Team not found")),
            other => panic!("expected GraphQLError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unauthorized_response_maps_to_authentication_failed() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/graphql")
            .with_status(401)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client.get_viewer().await.unwrap_err();

        assert!(matches!(err, Error::AuthenticationFailed(_)));
    }

    #[tokio::test]
    async fn rate_limited_response_maps_to_rate_limit_exceeded() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/graphql")
            .with_status(429)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client.get_viewer().await.unwrap_err();

        assert!(matches!(err, Error::RateLimitExceeded { .. }));
    }
}
