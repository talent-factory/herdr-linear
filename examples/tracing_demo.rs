//! Herdr Linear - Rust client for Linear.app GraphQL API
//!
//! This example demonstrates structured logging via `tracing` while exercising
//! viewer/teams/issues calls end to end.
//!
//! Run with: RUST_LOG=debug cargo run --example tracing_demo

use herdr_linear::LinearClient;
use std::env;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("herdr_linear=debug".parse()?))
        .init();

    info!("Starting Herdr Linear client");

    // Get API key from environment
    let api_key = env::var("LINEAR_API_KEY").map_err(|_| {
        error!("LINEAR_API_KEY environment variable not set");
        "LINEAR_API_KEY environment variable required"
    })?;

    // Create client
    let client = LinearClient::new(&api_key)?;
    info!("Successfully created Linear client");

    // Example 1: Get authenticated user
    match client.get_viewer().await {
        Ok(viewer) => {
            info!("Authenticated as: {} ({})", viewer.name, viewer.email);
        }
        Err(e) => {
            error!("Failed to get viewer: {}", e);
            return Err(e.into());
        }
    }

    // Example 2: Get teams
    match client.get_teams(Some(10), None).await {
        Ok(teams_conn) => {
            info!("Found {} teams", teams_conn.nodes.len());
            for team in teams_conn.nodes {
                info!("  - {} ({})", team.name, team.key);
            }
        }
        Err(e) => {
            error!("Failed to get teams: {}", e);
        }
    }

    // Example 3: Get issues (if teams exist)
    match client.get_issues(None, Some(5), None).await {
        Ok(issues_conn) => {
            info!("Found {} issues", issues_conn.nodes.len());
            for issue in issues_conn.nodes.iter().take(3) {
                info!(
                    "  - {} [{}] ({})",
                    issue.identifier, issue.title, issue.state.name
                );
            }
        }
        Err(e) => {
            error!("Failed to get issues: {}", e);
        }
    }

    info!("Herdr Linear client example completed");
    Ok(())
}
