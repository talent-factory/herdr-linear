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
