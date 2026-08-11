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

/// Number of attempts made for a single GraphQL operation before a
/// persistent `Error::RateLimitExceeded` is surfaced to the caller. Counts
/// the initial request, so `3` means up to 2 automatic retries. Not
/// currently overridable (unlike this file's `DEFAULT_*` constants, which
/// all have an options-struct override path), hence no `DEFAULT_` prefix.
const RATE_LIMIT_MAX_ATTEMPTS: u32 = 3;

/// Base delay for the exponential-backoff fallback used when a rate-limited
/// response doesn't carry a usable `retry_after_ms` (i.e. it's `0`).
/// Attempt `n` (1-indexed) waits `RATE_LIMIT_BACKOFF_BASE * 2^(n-1)`, capped
/// at `RATE_LIMIT_MAX_WAIT`.
const RATE_LIMIT_BACKOFF_BASE: Duration = Duration::from_millis(500);

/// Upper bound on any single rate-limit wait, whether sourced from the
/// server's `Retry-After` value or the exponential-backoff fallback. Guards
/// against a hostile/misconfigured `Retry-After` value — or a future,
/// larger `RATE_LIMIT_MAX_ATTEMPTS` — blocking the caller for an
/// unreasonable amount of time.
const RATE_LIMIT_MAX_WAIT: Duration = Duration::from_secs(60);

/// Default number of items requested per page by `get_all_*` calls that don't
/// override it — matches the default used by the single-page methods.
const DEFAULT_PAGE_SIZE: i32 = 50;

/// Default safety cap on the number of pages a `get_all_*` call will fetch
/// before aborting with `Error::InvalidRequest`, in case a misbehaving API
/// never reports `hasNextPage: false`.
const DEFAULT_MAX_PAGES: usize = 100;

/// Default safety cap on the total number of items a `get_all_*` call will
/// accumulate before aborting with `Error::InvalidRequest`. Checked only when
/// more pages remain, so a call that finishes exactly on (or slightly past,
/// by up to one page's worth of items) this cap on its final page still
/// succeeds — the cap guards against runaway pagination, not result size.
const DEFAULT_MAX_ITEMS: usize = 10_000;

/// Options controlling an auto-paginating `get_all_*` call.
///
/// All fields are optional; unset fields fall back to this crate's default
/// page size (50), max page count (100), and max item count (10,000)
/// respectively. The caps exist so a `get_all_*` call can't loop forever (or
/// exhaust memory) against an API that keeps reporting more pages than
/// expected.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaginationOptions {
    /// Number of items requested per page.
    pub page_size: Option<i32>,
    /// Maximum number of pages to fetch before aborting.
    pub max_pages: Option<usize>,
    /// Maximum number of items to accumulate before aborting.
    pub max_items: Option<usize>,
}

impl PaginationOptions {
    /// Override the page size requested per call. Must be positive (a
    /// non-positive value is rejected before any request is sent). Linear's
    /// API caps `first` at 250 regardless of what's requested here.
    pub fn with_page_size(mut self, page_size: i32) -> Self {
        self.page_size = Some(page_size);
        self
    }

    /// Override the max-pages safety cap.
    pub fn with_max_pages(mut self, max_pages: usize) -> Self {
        self.max_pages = Some(max_pages);
        self
    }

    /// Override the max-items safety cap.
    pub fn with_max_items(mut self, max_items: usize) -> Self {
        self.max_items = Some(max_items);
        self
    }
}

/// Build a filter matching issues belonging to `team_id`.
///
/// Shared by [`LinearClient::get_team_issues`] and
/// [`LinearClient::get_all_team_issues`] so the two stay in sync.
fn team_filter(team_id: &str) -> Value {
    json!({
        "team": {
            "id": {
                "eq": team_id
            }
        }
    })
}

/// Linear GraphQL API client
pub struct LinearClient {
    http_client: HttpClient,
    api_key: String,
    endpoint: String,
    rate_limit_retry: bool,
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
            rate_limit_retry: true,
        })
    }

    /// Create a client pointed at a custom endpoint. Used by this crate's own tests to target a
    /// mock server; kept `pub` (rather than `#[cfg(test)]`-gated) so the `herdr-linear` binary
    /// target's tests — a separate crate from this library's perspective — can do the same for
    /// `main.rs`'s `implement_one` integration tests, since a `#[cfg(test)]` item compiled for
    /// this crate's own test runs doesn't exist in the copy of this crate linked into another
    /// target's test binary.
    pub fn with_endpoint<S: Into<String>>(api_key: S, endpoint: String) -> Result<Self> {
        let mut client = Self::new(api_key)?;
        client.endpoint = endpoint;
        Ok(client)
    }

    /// Enables or disables automatic retry-with-backoff on
    /// `Error::RateLimitExceeded`. When disabled, a rate-limited response is
    /// surfaced to the caller immediately (fail-fast) instead of being
    /// retried.
    ///
    /// Retry is enabled by default; call `with_rate_limit_retry(false)` when
    /// the caller wants to own its own wait-and-retry (or backoff) strategy.
    pub fn with_rate_limit_retry(mut self, enabled: bool) -> Self {
        self.rate_limit_retry = enabled;
        self
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

    /// Get one page of teams
    pub async fn get_teams(
        &self,
        limit: Option<i32>,
        after: Option<String>,
    ) -> Result<Connection<Team>> {
        debug!("Fetching teams");
        let variables = json!({
            "first": limit.unwrap_or(DEFAULT_PAGE_SIZE),
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

    /// Fetch every team by transparently paging through [`Self::get_teams`]
    /// until `hasNextPage` is `false`.
    ///
    /// See [`PaginationOptions`] for page-size and safety-cap configuration.
    pub async fn get_all_teams(&self, options: PaginationOptions) -> Result<Vec<Team>> {
        self.paginate_all(options, move |page_size, after| {
            self.get_teams(Some(page_size), after)
        })
        .await
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
            "first": limit.unwrap_or(DEFAULT_PAGE_SIZE),
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

    /// Fetch every issue matching `filter` by transparently paging through
    /// [`Self::get_issues`] until `hasNextPage` is `false`.
    ///
    /// See [`PaginationOptions`] for page-size and safety-cap configuration.
    pub async fn get_all_issues(
        &self,
        filter: Option<Value>,
        options: PaginationOptions,
    ) -> Result<Vec<Issue>> {
        self.paginate_all(options, move |page_size, after| {
            self.get_issues(filter.clone(), Some(page_size), after)
        })
        .await
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
        self.get_issues(Some(team_filter(team_id)), limit, None)
            .await
    }

    /// Fetch every issue for a specific team by transparently paging through
    /// [`Self::get_all_issues`] with a team filter applied, until
    /// `hasNextPage` is `false`.
    ///
    /// See [`PaginationOptions`] for page-size and safety-cap configuration.
    pub async fn get_all_team_issues(
        &self,
        team_id: &str,
        options: PaginationOptions,
    ) -> Result<Vec<Issue>> {
        self.get_all_issues(Some(team_filter(team_id)), options)
            .await
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

    /// Get one page of projects
    ///
    /// # Arguments
    /// * `filter` - Optional filter criteria (JSON value)
    /// * `limit` - Max number of projects to return (default: 50)
    /// * `after` - Cursor for pagination
    pub async fn get_projects(
        &self,
        filter: Option<Value>,
        limit: Option<i32>,
        after: Option<String>,
    ) -> Result<Connection<Project>> {
        debug!("Fetching projects");
        let variables = json!({
            "first": limit.unwrap_or(DEFAULT_PAGE_SIZE),
            "after": after,
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

    /// Fetch every project matching `filter` by transparently paging through
    /// [`Self::get_projects`] until `hasNextPage` is `false`.
    ///
    /// See [`PaginationOptions`] for page-size and safety-cap configuration.
    pub async fn get_all_projects(
        &self,
        filter: Option<Value>,
        options: PaginationOptions,
    ) -> Result<Vec<Project>> {
        self.paginate_all(options, move |page_size, after| {
            self.get_projects(filter.clone(), Some(page_size), after)
        })
        .await
    }

    /// Get cycles for a team
    pub async fn get_cycles(&self, team_id: &str, limit: Option<i32>) -> Result<Connection<Cycle>> {
        debug!("Fetching cycles for team: {}", team_id);
        let variables = json!({
            "filter": {"team": {"id": {"eq": team_id}}},
            "first": limit.unwrap_or(DEFAULT_PAGE_SIZE)
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

    /// Internal method to execute GraphQL operations, transparently retrying
    /// on `Error::RateLimitExceeded` (unless disabled via
    /// [`with_rate_limit_retry`](Self::with_rate_limit_retry)).
    ///
    /// Waits the server-provided `retry_after_ms` between attempts, falling
    /// back to exponential backoff when that value is `0` (not usable), and
    /// clamping either source to [`RATE_LIMIT_MAX_WAIT`]. Gives up after
    /// [`RATE_LIMIT_MAX_ATTEMPTS`] attempts and returns the last
    /// `Error::RateLimitExceeded` unchanged.
    ///
    /// Retrying is safe even for mutations: `Error::RateLimitExceeded` is
    /// only ever produced by [`execute_graphql_once`](Self::execute_graphql_once)
    /// when the *rate limiter itself* rejected the request (HTTP 429, or
    /// HTTP 400 with a `RATELIMITED` GraphQL error code) — both signal that
    /// Linear never reached the resolver, so no mutation side effect can
    /// exist to duplicate. This assumption would need re-checking before
    /// ever retrying on other error kinds (5xx, network timeouts), where a
    /// mutation may already have been applied server-side.
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
        let kind = if is_mutation { "mutation" } else { "query" };

        debug!("Executing {kind} with payload: {payload}");

        let mut attempt: u32 = 1;

        loop {
            let result = self.execute_graphql_once(&payload).await;

            if let Err(Error::RateLimitExceeded { retry_after_ms }) = &result {
                if self.rate_limit_retry && attempt < RATE_LIMIT_MAX_ATTEMPTS {
                    let wait = Self::rate_limit_backoff(attempt, *retry_after_ms);
                    warn!(
                        "Rate limited by Linear API ({kind}, attempt {attempt}/{RATE_LIMIT_MAX_ATTEMPTS}), retrying in {}ms",
                        wait.as_millis()
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                } else if self.rate_limit_retry {
                    warn!(
                        "Rate limited by Linear API ({kind}): retry budget exhausted after {attempt} attempt(s), surfacing error"
                    );
                } else {
                    warn!(
                        "Rate limited by Linear API ({kind}, retry_after_ms={retry_after_ms}): automatic retry disabled, surfacing error"
                    );
                }
            }

            return result;
        }
    }

    /// Computes the wait before a rate-limit retry attempt (1-indexed).
    /// Uses `retry_after_ms` when it's usable (non-zero); otherwise falls
    /// back to exponential backoff seeded from `RATE_LIMIT_BACKOFF_BASE`.
    /// Either source is clamped to `RATE_LIMIT_MAX_WAIT`, and the fallback
    /// uses `saturating_mul` with a capped exponent so this never panics,
    /// regardless of how large `attempt` gets.
    fn rate_limit_backoff(attempt: u32, retry_after_ms: u64) -> Duration {
        let wait = if retry_after_ms > 0 {
            Duration::from_millis(retry_after_ms)
        } else {
            let exponent = attempt.saturating_sub(1).min(16);
            RATE_LIMIT_BACKOFF_BASE.saturating_mul(2u32.pow(exponent))
        };
        wait.min(RATE_LIMIT_MAX_WAIT)
    }

    /// Parses a `Retry-After` response header in `delay-seconds` form (RFC
    /// 9110 §10.2.3) into milliseconds. Returns `0` — "not usable, fall
    /// back to exponential backoff" — when the header is absent, not valid
    /// ASCII, or doesn't parse as a non-negative integer number of seconds.
    ///
    /// This intentionally does not parse the RFC-legal HTTP-date form of
    /// `Retry-After` (e.g. `Fri, 31 Dec 1999 23:59:59 GMT`): Linear's own
    /// rate-limiting docs don't document sending this header at all, so
    /// `delay-seconds` support here is best-effort rather than a documented
    /// contract worth a date-parsing dependency.
    fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> u64 {
        headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|secs| secs.saturating_mul(1000))
            .unwrap_or(0)
    }

    /// Returns `true` if `body` is a GraphQL error response containing at
    /// least one error whose `extensions.code` is `"RATELIMITED"` — the
    /// code Linear's GraphQL API uses (via an HTTP 400 response) to signal
    /// that a request was rejected by its rate limiter. Any other shape (a
    /// different `extensions.code`, no `extensions` at all, or a body that
    /// doesn't even parse as a GraphQL response) returns `false`, so
    /// ordinary HTTP 400s fall through to `Error::ApiError` unchanged.
    fn is_rate_limited_error_body(body: &str) -> bool {
        let Ok(parsed) = serde_json::from_str::<GraphQLResponse<Value>>(body) else {
            return false;
        };
        parsed.errors.is_some_and(|errors| {
            errors.iter().any(|e| {
                e.extensions
                    .as_ref()
                    .and_then(|ext| ext.get("code"))
                    .and_then(|code| code.as_str())
                    == Some("RATELIMITED")
            })
        })
    }

    /// Performs a single GraphQL request/response round-trip with no retry.
    async fn execute_graphql_once<T: serde::de::DeserializeOwned>(
        &self,
        payload: &Value,
    ) -> Result<T> {
        let response = self
            .http_client
            .post(&self.endpoint)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(payload)
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
                // Linear's documented rate-limit signal is the HTTP 400 +
                // `RATELIMITED` case below, but some deployments may front
                // the GraphQL API with an edge/CDN layer that uses the
                // standard HTTP 429 status — this branch is kept as a
                // defense-in-depth fallback for that case.
                let retry_after_ms = Self::parse_retry_after_ms(response.headers());
                debug!("Rate limited by Linear API via HTTP 429 (retry_after_ms={retry_after_ms})");
                Err(Error::RateLimitExceeded { retry_after_ms })
            }
            StatusCode::BAD_REQUEST => {
                // Linear's actual rate-limit signal: HTTP 400 with a
                // GraphQL error whose `extensions.code` is `RATELIMITED`.
                // See https://linear.app/developers/rate-limiting — other
                // causes of 400 fall through to the generic ApiError below.
                let retry_after_ms = Self::parse_retry_after_ms(response.headers());
                let body = response.text().await.unwrap_or_default();

                if Self::is_rate_limited_error_body(&body) {
                    debug!(
                        "Rate limited by Linear API via HTTP 400 + RATELIMITED (retry_after_ms={retry_after_ms})"
                    );
                    return Err(Error::RateLimitExceeded { retry_after_ms });
                }

                error!("HTTP error {}: {}", status, body);
                Err(api_error(format!("HTTP {}: {}", status, body)))
            }
            _ => {
                let body = response.text().await.unwrap_or_default();
                error!("HTTP error {}: {}", status, body);
                Err(api_error(format!("HTTP {}: {}", status, body)))
            }
        }
    }

    /// Repeatedly calls `fetch_page` with an advancing `after` cursor,
    /// accumulating `nodes` from each [`Connection<T>`] until
    /// `page_info.has_next_page` is `false`.
    ///
    /// This is a client-side loop built on top of the existing single-page
    /// methods; it does not change their behavior. See [`PaginationOptions`]
    /// for the page-size and safety-cap knobs.
    ///
    /// # Errors
    /// Returns `Error::InvalidRequest` if:
    /// - `options` resolves to a non-positive `page_size`, or a zero
    ///   `max_pages`/`max_items` — these can never yield a usable result, so
    ///   they're rejected before any request is sent.
    /// - `max_pages` is exceeded while more pages remain.
    /// - `max_items` is exceeded while more pages remain (a final page that
    ///   completes the result is still returned even if it pushes the total
    ///   past `max_items` — the cap guards against runaway pagination, not
    ///   result size).
    /// - the API reports `hasNextPage: true` without an `end_cursor` to
    ///   continue from — an API contract violation that would otherwise be
    ///   indistinguishable from a legitimately complete (but silently
    ///   truncated) result.
    async fn paginate_all<T, F, Fut>(
        &self,
        options: PaginationOptions,
        mut fetch_page: F,
    ) -> Result<Vec<T>>
    where
        F: FnMut(i32, Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<Connection<T>>>,
    {
        let page_size = options.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        let max_pages = options.max_pages.unwrap_or(DEFAULT_MAX_PAGES);
        let max_items = options.max_items.unwrap_or(DEFAULT_MAX_ITEMS);

        if page_size <= 0 {
            return Err(Error::InvalidRequest(format!(
                "auto-pagination rejected: page_size must be positive, got {page_size}"
            )));
        }
        if max_pages == 0 {
            return Err(Error::InvalidRequest(
                "auto-pagination rejected: max_pages must be at least 1".to_string(),
            ));
        }
        if max_items == 0 {
            return Err(Error::InvalidRequest(
                "auto-pagination rejected: max_items must be at least 1".to_string(),
            ));
        }

        let mut items: Vec<T> = Vec::new();
        let mut after: Option<String> = None;
        let mut pages_fetched = 0usize;

        loop {
            if pages_fetched >= max_pages {
                return Err(Error::InvalidRequest(format!(
                    "auto-pagination aborted: exceeded max_pages ({max_pages}) with more pages remaining ({} item(s) fetched so far)",
                    items.len()
                )));
            }

            let connection = fetch_page(page_size, after).await?;
            pages_fetched += 1;
            items.extend(connection.nodes);

            if !connection.page_info.has_next_page {
                // Complete: return everything gathered, even if the final
                // page happened to push the total past `max_items` — the cap
                // exists to bound an unbounded/misbehaving loop, not to
                // reject an otherwise-correct, already-finished result.
                return Ok(items);
            }

            if items.len() > max_items {
                return Err(Error::InvalidRequest(format!(
                    "auto-pagination aborted: exceeded max_items ({max_items}) after {pages_fetched} page(s), with more pages remaining"
                )));
            }

            after = match connection.page_info.end_cursor {
                Some(cursor) => Some(cursor),
                // A well-behaved API always pairs `hasNextPage: true` with a
                // cursor to continue from. Treat the absence of one as a
                // detected API contract violation and fail loudly, rather
                // than silently returning a truncated result as `Ok`.
                None => {
                    return Err(Error::InvalidRequest(format!(
                        "auto-pagination aborted: API reported hasNextPage=true with no end_cursor after {pages_fetched} page(s) ({} item(s) fetched, result may be incomplete)",
                        items.len()
                    )));
                }
            };
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
        // Also covers "a request that succeeds on the first try incurs no
        // retry overhead/delay": `mock.assert_async()` below requires
        // exactly 1 hit (mockito's default), so a spurious retry would fail
        // this test.
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
    async fn rate_limited_request_succeeds_after_one_retry() {
        // Registered first: mockito prefers a still-"missing-hits" mock in
        // registration order, so the first request hits this 429 mock and
        // the second (retried) request falls through to `ok` below.
        // `Retry-After: 1` keeps the real wait the retry performs short
        // enough for a unit test. (`#[tokio::test(start_paused = true)]`
        // was tried to make this instant, but it produced spurious
        // `NetworkError`s against mockito's real I/O, so this test still
        // pays a real ~1s sleep.)
        let mut server = mockito::Server::new_async().await;
        let rate_limited = server
            .mock("POST", "/graphql")
            .with_status(429)
            .with_header("retry-after", "1")
            .expect(1)
            .create_async()
            .await;

        let ok = server
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
            .expect(1)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let viewer = client.get_viewer().await.unwrap();

        assert_eq!(viewer.name, "Alice");
        rate_limited.assert_async().await;
        ok.assert_async().await;
    }

    #[tokio::test]
    async fn repeated_rate_limiting_exhausts_the_retry_budget_and_surfaces_the_error() {
        // No `Retry-After` header, so each wait also exercises the
        // exponential-backoff fallback (500ms, 1000ms — short enough for a
        // unit test).
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(429)
            .expect(RATE_LIMIT_MAX_ATTEMPTS as usize)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client.get_viewer().await.unwrap_err();

        assert!(matches!(err, Error::RateLimitExceeded { .. }));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn rate_limit_retry_can_be_opted_out() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(429)
            .expect(1)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap()
                .with_rate_limit_retry(false);

        let err = client.get_viewer().await.unwrap_err();

        assert!(matches!(err, Error::RateLimitExceeded { .. }));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn rate_limited_mutation_succeeds_after_retry() {
        // Proves retry isn't silently skipped for mutations: `is_mutation`
        // is passed into `execute_graphql` (it only affects the log line's
        // "query"/"mutation" label) but, by design — see `execute_graphql`'s
        // doc comment — never gates the retry decision.
        let mut server = mockito::Server::new_async().await;
        let rate_limited = server
            .mock("POST", "/graphql")
            .with_status(429)
            .with_header("retry-after", "1")
            .expect(1)
            .create_async()
            .await;

        let ok = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "commentCreate": {
                            "comment": {
                                "id": "comment-1",
                                "body": "hello",
                                "user": {
                                    "id": "user-1",
                                    "email": "alice@example.com",
                                    "name": "Alice",
                                    "avatarUrl": null,
                                    "createdAt": "2026-01-01T00:00:00Z",
                                    "updatedAt": "2026-01-01T00:00:00Z"
                                },
                                "createdAt": "2026-01-01T00:00:00Z",
                                "updatedAt": "2026-01-01T00:00:00Z"
                            }
                        }
                    },
                    "errors": null
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let comment = client.add_comment("issue-1", "hello").await.unwrap();

        assert_eq!(comment.body, "hello");
        rate_limited.assert_async().await;
        ok.assert_async().await;
    }

    #[tokio::test]
    async fn rate_limited_via_http_400_with_ratelimited_code_retries_and_succeeds() {
        // Linear's actual, documented rate-limit signal:
        // https://linear.app/developers/rate-limiting
        let mut server = mockito::Server::new_async().await;
        let rate_limited = server
            .mock("POST", "/graphql")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_header("retry-after", "0")
            .with_body(
                json!({
                    "data": null,
                    "errors": [{
                        "message": "Rate limit exceeded",
                        "extensions": { "code": "RATELIMITED" }
                    }]
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let ok = server
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
            .expect(1)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let viewer = client.get_viewer().await.unwrap();

        assert_eq!(viewer.name, "Alice");
        rate_limited.assert_async().await;
        ok.assert_async().await;
    }

    #[tokio::test]
    async fn http_400_without_ratelimited_code_maps_to_api_error_not_rate_limited() {
        // An ordinary HTTP 400 (bad request shape, validation error, etc.)
        // must NOT be misclassified as a rate limit — no retry, no
        // RateLimitExceeded.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": null,
                    "errors": [{
                        "message": "Cannot query field \"bogus\"",
                        "extensions": { "code": "GRAPHQL_VALIDATION_FAILED" }
                    }]
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client.get_viewer().await.unwrap_err();

        assert!(matches!(err, Error::ApiError { .. }));
        mock.assert_async().await;
    }

    #[test]
    fn rate_limit_backoff_uses_the_servers_retry_after_when_usable() {
        assert_eq!(
            LinearClient::rate_limit_backoff(1, 250),
            Duration::from_millis(250)
        );
        assert_eq!(
            LinearClient::rate_limit_backoff(2, 250),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn rate_limit_backoff_falls_back_to_exponential_backoff_when_retry_after_is_unusable() {
        assert_eq!(
            LinearClient::rate_limit_backoff(1, 0),
            RATE_LIMIT_BACKOFF_BASE
        );
        assert_eq!(
            LinearClient::rate_limit_backoff(2, 0),
            RATE_LIMIT_BACKOFF_BASE * 2
        );
        assert_eq!(
            LinearClient::rate_limit_backoff(3, 0),
            RATE_LIMIT_BACKOFF_BASE * 4
        );
    }

    #[test]
    fn rate_limit_backoff_treats_attempt_zero_like_attempt_one() {
        // Not reachable via the real retry loop (which always starts at
        // `attempt = 1`), but `rate_limit_backoff` is a free function with
        // no enforced precondition — document the boundary explicitly.
        assert_eq!(
            LinearClient::rate_limit_backoff(0, 0),
            RATE_LIMIT_BACKOFF_BASE
        );
    }

    #[test]
    fn rate_limit_backoff_clamps_both_sources_to_the_max_wait_cap() {
        // A huge server-provided `Retry-After` is clamped...
        assert_eq!(
            LinearClient::rate_limit_backoff(1, u64::MAX),
            RATE_LIMIT_MAX_WAIT
        );
        // ...and so is a huge exponential-backoff fallback — and neither
        // panics, however large `attempt` gets.
        assert_eq!(
            LinearClient::rate_limit_backoff(u32::MAX, 0),
            RATE_LIMIT_MAX_WAIT
        );
    }

    #[test]
    fn parse_retry_after_ms_uses_a_valid_delay_seconds_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 5000);
    }

    #[test]
    fn parse_retry_after_ms_trims_surrounding_whitespace() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, " 5 ".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 5000);
    }

    #[test]
    fn parse_retry_after_ms_is_zero_when_header_is_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 0);
    }

    #[test]
    fn parse_retry_after_ms_is_zero_for_non_numeric_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 0);
    }

    #[test]
    fn parse_retry_after_ms_is_zero_for_decimal_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "1.5".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 0);
    }

    #[test]
    fn parse_retry_after_ms_is_zero_for_negative_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "-5".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 0);
    }

    #[test]
    fn parse_retry_after_ms_is_zero_for_an_empty_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "".parse().unwrap());
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), 0);
    }

    #[test]
    fn parse_retry_after_ms_saturates_instead_of_overflowing_on_a_huge_value() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            u64::MAX.to_string().parse().unwrap(),
        );
        assert_eq!(LinearClient::parse_retry_after_ms(&headers), u64::MAX);
    }

    #[test]
    fn is_rate_limited_error_body_detects_the_ratelimited_extension_code() {
        let body = json!({
            "data": null,
            "errors": [{"message": "Rate limit exceeded", "extensions": {"code": "RATELIMITED"}}]
        })
        .to_string();
        assert!(LinearClient::is_rate_limited_error_body(&body));
    }

    #[test]
    fn is_rate_limited_error_body_is_false_for_other_error_codes() {
        let body = json!({
            "data": null,
            "errors": [{"message": "nope", "extensions": {"code": "SOMETHING_ELSE"}}]
        })
        .to_string();
        assert!(!LinearClient::is_rate_limited_error_body(&body));
    }

    #[test]
    fn is_rate_limited_error_body_is_false_for_unparseable_bodies() {
        assert!(!LinearClient::is_rate_limited_error_body("not json"));
        assert!(!LinearClient::is_rate_limited_error_body(""));
    }

    #[tokio::test]
    async fn get_projects_parses_a_project_with_a_lead() {
        // Regression test: `Project.lead` deserializes into `User`, which requires
        // `avatarUrl`/`createdAt`/`updatedAt` alongside `id`/`email`/`name` (no
        // `#[serde(default)]`). QUERY_PROJECTS's `lead` sub-selection previously only
        // asked for `id`/`name`/`email`, so a real (well-formed, matching what was
        // actually selected) API response failed to parse with "missing field
        // `createdAt`" — this mock mirrors the full `User` shape the query now selects.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "projects": {
                            "nodes": [{
                                "id": "project-1",
                                "name": "herdr-linear",
                                "description": "",
                                "url": "https://linear.app/talent-factory/project/herdr-linear",
                                "status": { "id": "status-1", "name": "Backlog", "type": "backlog" },
                                "createdAt": "2026-01-01T00:00:00Z",
                                "updatedAt": "2026-01-01T00:00:00Z",
                                "startDate": null,
                                "targetDate": null,
                                "lead": {
                                    "id": "user-1",
                                    "name": "Daniel Senften",
                                    "email": "daniel@example.com",
                                    "avatarUrl": null,
                                    "createdAt": "2026-01-01T00:00:00Z",
                                    "updatedAt": "2026-01-01T00:00:00Z"
                                }
                            }],
                            "pageInfo": {
                                "hasNextPage": false,
                                "hasPreviousPage": false,
                                "startCursor": null,
                                "endCursor": null
                            }
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

        let connection = client
            .get_projects(None, Some(50), None)
            .await
            .expect("a project whose lead has the full User shape should parse");

        assert_eq!(connection.nodes.len(), 1);
        assert_eq!(
            connection.nodes[0].lead.as_ref().map(|u| u.name.as_str()),
            Some("Daniel Senften")
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_issues_parses_an_issue_with_a_cycle() {
        // Regression test: `Issue.cycle` deserializes into `Cycle`, whose `team` field is
        // required (non-`Option`) — but QUERY_ISSUES's `cycle` sub-selection never asked
        // for it. Any issue actually assigned to a cycle would fail to parse with
        // "missing field `team`", breaking the whole `get_issues()` call (a single
        // unparseable node fails the surrounding `Vec<Issue>` deserialization). This
        // mirrors a realistic response for an issue with a cycle set.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "issues": {
                            "nodes": [{
                                "id": "issue-1",
                                "identifier": "ENG-1",
                                "title": "Test issue",
                                "description": null,
                                "priority": 0,
                                "estimate": null,
                                "url": "https://linear.app/team/issue/ENG-1",
                                "createdAt": "2026-01-01T00:00:00Z",
                                "updatedAt": "2026-01-01T00:00:00Z",
                                "startedAt": null,
                                "completedAt": null,
                                "state": { "id": "state-1", "name": "In Progress", "type": "started" },
                                "team": {
                                    "id": "team-1", "key": "ENG", "name": "Engineering",
                                    "description": null,
                                    "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
                                },
                                "assignee": null,
                                "creator": null,
                                "cycle": {
                                    "id": "cycle-1",
                                    "number": 12,
                                    "name": "Sprint 12",
                                    "team": {
                                        "id": "team-1", "key": "ENG", "name": "Engineering",
                                        "description": null,
                                        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
                                    },
                                    "createdAt": "2026-01-01T00:00:00Z",
                                    "updatedAt": "2026-01-01T00:00:00Z",
                                    "startsAt": "2026-01-01T00:00:00Z",
                                    "endsAt": "2026-01-15T00:00:00Z",
                                    "completedAt": null
                                },
                                "project": null,
                                "labels": { "nodes": [] }
                            }],
                            "pageInfo": {
                                "hasNextPage": false,
                                "hasPreviousPage": false,
                                "startCursor": null,
                                "endCursor": null
                            }
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

        let connection = client
            .get_issues(None, Some(50), None)
            .await
            .expect("an issue whose cycle has the full Cycle shape should parse");

        assert_eq!(connection.nodes.len(), 1);
        assert_eq!(
            connection.nodes[0]
                .cycle
                .as_ref()
                .map(|c| c.team.key.as_str()),
            Some("ENG")
        );
        mock.assert_async().await;
    }

    // --- get_all_* auto-pagination helpers -------------------------------

    /// Minimal but schema-complete `Issue` JSON matching everything
    /// `QUERY_ISSUES` selects, so it deserializes into `Issue` successfully.
    fn sample_issue_json(id: &str, identifier: &str) -> Value {
        json!({
            "id": id,
            "identifier": identifier,
            "title": "Test issue",
            "description": null,
            "priority": 0,
            "estimate": null,
            "url": format!("https://linear.app/team/issue/{identifier}"),
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "startedAt": null,
            "completedAt": null,
            "state": { "id": "state-1", "name": "Todo", "type": "unstarted" },
            "team": {
                "id": "team-1", "key": "ENG", "name": "Engineering",
                "description": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "assignee": null,
            "creator": null,
            "cycle": null,
            "project": null,
            "labels": { "nodes": [] }
        })
    }

    /// Minimal `Team` JSON matching `QUERY_TEAMS`'s selection.
    fn sample_team_json(id: &str, key: &str) -> Value {
        json!({
            "id": id,
            "key": key,
            "name": key,
            "description": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        })
    }

    fn issues_page(nodes: Value, has_next_page: bool, end_cursor: Option<&str>) -> String {
        json!({
            "data": {
                "issues": {
                    "nodes": nodes,
                    "pageInfo": {
                        "hasNextPage": has_next_page,
                        "hasPreviousPage": false,
                        "startCursor": null,
                        "endCursor": end_cursor
                    }
                }
            },
            "errors": null
        })
        .to_string()
    }

    #[tokio::test]
    async fn get_all_issues_returns_empty_vec_for_an_empty_result() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(json!([]), false, None))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let issues = client
            .get_all_issues(None, PaginationOptions::default())
            .await
            .unwrap();

        assert!(issues.is_empty());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_follows_the_cursor_across_multiple_pages() {
        let mut server = mockito::Server::new_async().await;

        let page1 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":null"#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-1", "ENG-1")]),
                true,
                Some("cursor-1"),
            ))
            .create_async()
            .await;

        let page2 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":"cursor-1""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-2", "ENG-2")]),
                false,
                None,
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let issues = client
            .get_all_issues(None, PaginationOptions::default())
            .await
            .unwrap();

        let identifiers: Vec<&str> = issues.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(identifiers, vec!["ENG-1", "ENG-2"]);
        page1.assert_async().await;
        page2.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_aborts_when_the_page_cap_is_hit() {
        let mut server = mockito::Server::new_async().await;
        // Always reports another page — mimics a misbehaving API that never
        // reports `hasNextPage: false`.
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-1", "ENG-1")]),
                true,
                Some("cursor-1"),
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default().with_max_pages(1))
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => assert!(msg.contains("max_pages")),
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        // Aborts before issuing a second request, past the one-page cap.
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_aborts_when_the_item_cap_is_hit() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([
                    sample_issue_json("issue-1", "ENG-1"),
                    sample_issue_json("issue-2", "ENG-2")
                ]),
                true,
                Some("cursor-1"),
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default().with_max_items(1))
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => assert!(msg.contains("max_items")),
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_teams_honors_a_custom_page_size_and_stops_on_a_single_page() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            // PartialJson matches on structure/value regardless of key order,
            // unlike a literal-position regex — robust even if serde_json's
            // key ordering ever changes (e.g. the `preserve_order` feature).
            .match_body(mockito::Matcher::PartialJson(json!({
                "variables": { "first": 7 }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": {
                        "teams": {
                            "nodes": [sample_team_json("team-1", "ENG")],
                            "pageInfo": {
                                "hasNextPage": false,
                                "hasPreviousPage": false,
                                "startCursor": null,
                                "endCursor": null
                            }
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

        let teams = client
            .get_all_teams(PaginationOptions::default().with_page_size(7))
            .await
            .unwrap();

        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].key, "ENG");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_team_issues_scopes_the_request_to_the_team() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(
                r#""filter":\{"team":\{"id":\{"eq":"team-1"\}\}\}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-1", "ENG-1")]),
                false,
                None,
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let issues = client
            .get_all_team_issues("team-1", PaginationOptions::default())
            .await
            .unwrap();

        assert_eq!(issues.len(), 1);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_projects_paginates_through_get_projects() {
        let mut server = mockito::Server::new_async().await;

        fn projects_page(node_id: &str, has_next_page: bool, end_cursor: Option<&str>) -> String {
            json!({
                "data": {
                    "projects": {
                        "nodes": [{
                            "id": node_id,
                            "name": "herdr-linear",
                            "description": null,
                            "url": "https://linear.app/talent-factory/project/herdr-linear",
                            "leadId": null,
                            "lead": null,
                            "status": {
                                "id": "status-1",
                                "name": "Backlog",
                                "type": "backlog"
                            },
                            "createdAt": "2026-01-01T00:00:00Z",
                            "updatedAt": "2026-01-01T00:00:00Z",
                            "startDate": null,
                            "targetDate": null
                        }],
                        "pageInfo": {
                            "hasNextPage": has_next_page,
                            "hasPreviousPage": false,
                            "startCursor": null,
                            "endCursor": end_cursor
                        }
                    }
                },
                "errors": null
            })
            .to_string()
        }

        let page1 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":null"#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(projects_page("project-1", true, Some("cursor-1")))
            .create_async()
            .await;

        let page2 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":"cursor-1""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(projects_page("project-2", false, None))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let projects = client
            .get_all_projects(None, PaginationOptions::default())
            .await
            .unwrap();

        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["project-1", "project-2"]);
        page1.assert_async().await;
        page2.assert_async().await;
    }

    // --- get_all_* auto-pagination: edge cases added on review -----------

    #[tokio::test]
    async fn get_all_issues_errors_when_has_next_page_is_true_but_end_cursor_is_missing() {
        // API contract violation: `hasNextPage: true` must always be paired
        // with an `endCursor` to continue from. Regression test for the fix
        // that turned this from a silent `Ok(items)` truncation into a loud
        // `Err` — see `paginate_all`'s final `match` arm.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-1", "ENG-1")]),
                true,
                None,
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default())
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => {
                assert!(msg.contains("end_cursor"), "unexpected message: {msg}")
            }
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        // Aborts rather than issuing a second, cursor-less request.
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_requests_the_default_page_size_on_the_wire() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::PartialJson(json!({
                "variables": { "first": DEFAULT_PAGE_SIZE }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(json!([]), false, None))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        client
            .get_all_issues(None, PaginationOptions::default())
            .await
            .unwrap();

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_succeeds_when_the_final_page_lands_exactly_on_max_items() {
        // The final, completing page pushes `items.len()` to exactly
        // `max_items` — this must succeed, not abort: the cap guards against
        // runaway pagination, not against a correctly-sized finished result.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([
                    sample_issue_json("issue-1", "ENG-1"),
                    sample_issue_json("issue-2", "ENG-2")
                ]),
                false,
                None,
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let issues = client
            .get_all_issues(None, PaginationOptions::default().with_max_items(2))
            .await
            .unwrap();

        assert_eq!(issues.len(), 2);
        mock.assert_async().await;
    }

    // Note: `get_all_issues_aborts_when_the_item_cap_is_hit` (above) already
    // covers "still aborts when max_items is exceeded and more pages
    // remain" — that's the strictly-over-cap counterpart to the
    // exactly-on-cap success case just above.

    #[tokio::test]
    async fn get_all_issues_succeeds_when_the_last_page_lands_exactly_on_max_pages() {
        let mut server = mockito::Server::new_async().await;

        let page1 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":null"#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-1", "ENG-1")]),
                true,
                Some("cursor-1"),
            ))
            .create_async()
            .await;

        let page2 = server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(r#""after":"cursor-1""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json("issue-2", "ENG-2")]),
                false,
                None,
            ))
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let issues = client
            .get_all_issues(None, PaginationOptions::default().with_max_pages(2))
            .await
            .unwrap();

        assert_eq!(issues.len(), 2);
        page1.assert_async().await;
        page2.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_rejects_a_zero_max_pages_without_making_any_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(json!([]), false, None))
            .expect(0)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default().with_max_pages(0))
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => assert!(msg.contains("max_pages")),
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_rejects_a_zero_max_items_without_making_any_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(json!([]), false, None))
            .expect(0)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default().with_max_items(0))
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => assert!(msg.contains("max_items")),
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_all_issues_rejects_a_non_positive_page_size_without_making_any_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/graphql")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(json!([]), false, None))
            .expect(0)
            .create_async()
            .await;

        let client =
            LinearClient::with_endpoint("lin_api_test", format!("{}/graphql", server.url()))
                .unwrap();

        let err = client
            .get_all_issues(None, PaginationOptions::default().with_page_size(0))
            .await
            .unwrap_err();

        match err {
            Error::InvalidRequest(msg) => assert!(msg.contains("page_size")),
            other => panic!("expected Error::InvalidRequest, got {other:?}"),
        }
        mock.assert_async().await;
    }
}
