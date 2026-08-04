//! # Herdr Linear
//!
//! A Rust-native client for Linear.app's GraphQL API, designed as a plugin for Herdr.
//! Provides full access to Linear's workflow management capabilities.
//!
//! ## Quick Start
//!
//! ```no_run
//! use herdr_linear::LinearClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let api_key = "lin_api_YOUR_KEY_HERE";
//!     let client = LinearClient::new(api_key)?;
//!
//!     // Query viewer (authenticated user)
//!     let viewer = client.get_viewer().await?;
//!     println!("Authenticated as: {}", viewer.name);
//!
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod error;
pub mod models;
pub mod queries;

pub use client::LinearClient;
pub use error::{Error, Result};
pub use models::*;
