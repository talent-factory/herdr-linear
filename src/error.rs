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

    /// A `herdr agent start <name> ...` call rejected because `name` collides with an
    /// already-running agent tab (`error.code == "agent_name_taken"`, TF-590). Carries herdr's
    /// own `message` (so a caller that gives up still has herdr's original explanation to show,
    /// not just a generic collision notice) and its suggested `candidates` (may be empty if
    /// herdr's response didn't include any) so [`crate::plugin::herdr_cli::agent_start`]'s retry
    /// logic can pick one automatically instead of surfacing this raw collision to the user.
    /// Kept distinct from [`Error::Internal`] for the same reason as
    /// [`Error::MissingResultField`]: the retry decision matches on the variant, not a substring
    /// of the formatted message. Only `agent_start` ever sees this variant — see
    /// `crate::plugin::herdr_cli::interpret_output`'s docs for why the mapping is scoped to
    /// that one call path rather than applied to every `herdr` subcommand.
    #[error(
        "{message}{}",
        if candidates.is_empty() {
            String::new()
        } else {
            format!("; candidates: {}", candidates.join(", "))
        }
    )]
    AgentNameTaken {
        message: String,
        candidates: Vec<String>,
    },
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

    #[test]
    fn agent_name_taken_display_omits_candidates_suffix_when_empty() {
        let err = Error::AgentNameTaken {
            message: "agent name hr is already used".to_string(),
            candidates: vec![],
        };
        assert_eq!(err.to_string(), "agent name hr is already used");
    }

    #[test]
    fn agent_name_taken_display_appends_candidates_when_present() {
        let err = Error::AgentNameTaken {
            message: "agent name hr is already used".to_string(),
            candidates: vec!["hr-2".to_string(), "hr-3".to_string()],
        };
        assert_eq!(
            err.to_string(),
            "agent name hr is already used; candidates: hr-2, hr-3"
        );
    }
}
