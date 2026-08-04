# Herdr Linear

A **pure Rust** client for [Linear.app](https://linear.app)'s GraphQL API. Designed as a plugin for [Herdr](https://herdr.dev), with zero dependencies on Node.js or TypeScript.

## Features

✅ **Full GraphQL API Coverage**
- Query teams, issues, projects, cycles, and workflow states
- Create and update issues
- Add comments to issues
- Paginated queries with cursor support

✅ **Type-Safe**
- Fully typed models matching Linear's schema
- Compile-time error checking

✅ **Async/Await**
- Built on tokio for high-performance async operations
- Non-blocking I/O throughout

✅ **Error Handling**
- Comprehensive error types with context
- Automatic rate limit detection

✅ **Logging**
- Structured logging with tracing
- Debug-level operation tracking

## Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
herdr-linear = { path = "../herdr-linear" }
tokio = { version = "1", features = ["full"] }
```

### Basic Usage

```rust
use herdr_linear::LinearClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client with API key
    let api_key = "lin_api_YOUR_KEY_HERE";
    let client = LinearClient::new(api_key)?;

    // Get authenticated user
    let viewer = client.get_viewer().await?;
    println!("Authenticated as: {}", viewer.name);

    // Get all teams
    let teams = client.get_teams(Some(50), None).await?;
    for team in teams.nodes {
        println!("Team: {} ({})", team.name, team.key);
    }

    // Get issues for a team
    let issues = client.get_team_issues("team_id", Some(20)).await?;
    for issue in issues.nodes {
        println!("Issue: {} - {}", issue.identifier, issue.title);
    }

    // Create an issue
    let new_issue = client
        .create_issue("New feature", "team_id", Some("Description here"), Some(2))
        .await?;
    println!("Created: {}", new_issue.identifier);

    Ok(())
}
```

### Get Linear API Key

1. Go to [Linear Settings](https://linear.app/settings/api)
2. Create a Personal API key
3. Copy the key (format: `lin_api_*`)
4. Set as environment variable: `export LINEAR_API_KEY=lin_api_your_key`

## API Methods

### User & Authentication

```rust
client.get_viewer().await?  // Get current authenticated user
```

### Teams

```rust
client.get_teams(limit, after).await?           // List all teams
client.get_team("team_id").await?               // Get single team
client.get_team_issues("team_id", limit).await? // Get team's issues
```

### Issues

```rust
client.get_issues(filter, limit, after).await?  // List issues (with optional filter)
client.get_issue("issue_id").await?             // Get single issue
client.create_issue(title, team_id, description, priority).await?  // Create issue
client.update_issue("issue_id", updates).await? // Update issue
client.add_comment("issue_id", "body").await?   // Add comment to issue
```

### Projects & Cycles

```rust
client.get_projects(filter, limit).await?       // List projects
client.get_cycles("team_id", limit).await?      // Get team's cycles
client.get_workflow_states("team_id").await?    // Get team's workflow states
```

### Raw Queries

```rust
// Execute raw GraphQL query
let response = client.query::<MyType>(query_string, variables).await?;

// Execute raw GraphQL mutation
let response = client.mutate::<MyType>(mutation_string, variables).await?;
```

## Project Structure

```
herdr-linear/
├── src/
│   ├── lib.rs              # Library root
│   ├── main.rs             # Example usage & CLI entry
│   ├── client.rs           # LinearClient implementation
│   ├── models.rs           # Type definitions
│   ├── queries.rs          # GraphQL query strings
│   └── error.rs            # Error types
├── Cargo.toml
└── README.md
```

## Environment Variables

```bash
# Required
export LINEAR_API_KEY=lin_api_your_key_here

# Optional - Enable debug logging
export RUST_LOG=herdr_linear=debug
```

## Development

### Building

```bash
cd ~/GitRepository/herdr-linear
cargo build
```

### Running Examples

```bash
export LINEAR_API_KEY=lin_api_your_key
export RUST_LOG=debug

cargo run --bin herdr-linear
```

### Running Tests

```bash
cargo test
```

### Logging

The crate uses `tracing` for structured logging. Enable with environment variables:

```bash
# Debug level for herdr-linear
export RUST_LOG=herdr_linear=debug

# All debug logs
export RUST_LOG=debug

# JSON output
RUST_LOG=info cargo run --bin herdr-linear 2>&1 | jq
```

## Error Handling

All errors are of type `herdr_linear::Error`:

```rust
use herdr_linear::Error;

match client.get_viewer().await {
    Ok(user) => println!("User: {}", user.name),
    Err(Error::AuthenticationFailed(msg)) => eprintln!("Auth failed: {}", msg),
    Err(Error::RateLimitExceeded { retry_after_ms }) => {
        eprintln!("Rate limited, retry after {}ms", retry_after_ms);
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

## Issues & Feature Requests

Issues are tracked in Linear at:
https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview

## License

Dual-licensed under MIT OR Apache-2.0

## Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/your-feature`
3. Commit changes: `git commit -am 'Add feature'`
4. Push to branch: `git push origin feature/your-feature`
5. Submit a pull request

## Related Projects

- [Herdr](https://herdr.dev) - Task management & productivity platform
- [Linear](https://linear.app) - Issue tracking for software teams
- [Linear SDK (TypeScript)](https://github.com/linear/linear/tree/master/packages/sdk)

## Maintenance

Maintained by [Talent Factory GmbH](https://talent-factory.xyz)

---

**Status**: 🚀 Active Development

Last updated: 2026-08-04
