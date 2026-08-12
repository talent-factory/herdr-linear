//! Error types for Herdr Linear client

use thiserror::Error;

/// Result type alias for Herdr Linear operations
pub type Result<T> = std::result::Result<T, Error>;

/// Error types returned by Herdr Linear client
#[derive(Error, Debug)]
pub enum Error {
    #[error("Linear API error: {message}")]
    ApiError { message: String },

    #[error("Invalid API key provided")]
    InvalidApiKey,

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("GraphQL error: {0}")]
    GraphQLError(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Rate limit exceeded: {retry_after_ms}ms")]
    RateLimitExceeded { retry_after_ms: u64 },

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// A `herdr` CLI response that parsed as JSON, exited 0, and carried no top-level `error`
    /// body, but was missing the `result` field its own schema declares required. Kept as a
    /// distinct variant (rather than folded into [`Error::Internal`]) so callers like
    /// [`crate::plugin::herdr_cli::agent_wait`]'s retry-on-this-specific-bug logic can match on
    /// the variant itself instead of pattern-matching a substring of the formatted message —
    /// the message text can change (rewording, localization) without silently breaking that
    /// retry decision.
    #[error("{0}")]
    MissingResultField(String),

    /// herdr reported that the target pane is not (yet) tracked as an agent. Distinct from
    /// [`Error::Internal`] so `agent_wait` can poll: after `pane_run` types a launch command,
    /// herdr needs a moment to observe the resulting process and identify it as a coding agent
    /// (especially when the command is a shell alias/wrapper).
    #[error("{0}")]
    AgentNotFound(String),
}

/// Helper function to create GraphQL error responses
pub fn graphql_error<S: Into<String>>(msg: S) -> Error {
    Error::GraphQLError(msg.into())
}

/// Helper function to create API error responses
pub fn api_error<S: Into<String>>(msg: S) -> Error {
    Error::ApiError {
        message: msg.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_error_wraps_message_in_graphql_error_variant() {
        let err = graphql_error("boom");
        assert!(matches!(err, Error::GraphQLError(msg) if msg == "boom"));
    }

    #[test]
    fn api_error_wraps_message_in_api_error_variant() {
        let err = api_error("boom");
        assert!(matches!(err, Error::ApiError { message } if message == "boom"));
    }

    #[test]
    fn rate_limit_exceeded_display_includes_retry_after() {
        let err = Error::RateLimitExceeded {
            retry_after_ms: 5000,
        };
        assert_eq!(err.to_string(), "Rate limit exceeded: 5000ms");
    }
}
