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
client.get_projects(filter, limit, after).await? // List projects
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

cargo run --example tracing_demo
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
RUST_LOG=info cargo run --example tracing_demo 2>&1 | jq
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

## Herdr Plugin

`herdr-linear` also ships as a [Herdr](https://herdr.dev) plugin: a "My Issues"
panel you can open as a split pane or a tab from inside a Herdr session. Browsing
issues is read-only; pressing `<Enter>` on a selected issue is not — see "Use" below.

### Install

```bash
herdr plugin install talent-factory/herdr-linear
```

For local development, use `herdr plugin link /path/to/herdr-linear` instead.

### Configure

Set your Linear API key in the plugin's config file:

```bash
mkdir -p "$(herdr plugin config-dir herdr-linear)"
echo 'api_key = "lin_api_your_key_here"' > "$(herdr plugin config-dir herdr-linear)/config.toml"
```

Or export `LINEAR_API_KEY` in the environment Herdr runs in.

If "Project Issues" can't match your repo to a Linear project by name (see "Use" below),
add a repo-scoped override to the same `config.toml`:

```toml
[project_overrides]
"your-repo-name" = "linear-project-id"
```

This file is shared by every repo/workspace that opens this plugin's panel (there's one
`config.toml` per plugin *installation*, not per repo), so `project_overrides` is a table
keyed by repo name rather than a single value — an entry for one repo never affects how
another repo resolves. You don't need to work out the repo name or project id yourself:
pressing `c` on any Linear error screen (no project matches, multiple projects match, etc.)
opens this file with your OS's default handler for `.toml` files (creating it if it doesn't
exist yet), and the error text itself shows the exact snippet to paste in, with your repo
name already filled in.

Optionally set `agent_command` in the same `config.toml` to choose the coding agent started
when you implement an issue (see "Use" below). If set, it **always** wins. If unset, the
plugin looks at your other open herdr tabs and reuses whatever coding agent you're already
running there; if none are open either, it falls back to `"hr"`, a personal shell alias — it
won't exist for other users, so either set `agent_command` yourself or define an `hr`
alias/function in your shell.

(Earlier versions preferred the other-open-tabs guess over an explicit `agent_command`. That
was reversed: herdr's tab list can only report the underlying binary a pane runs — e.g.
`"claude"` — never the alias that launched it, so a pane started via `hr` looks identical to
one started bare. Under the old precedence, `agent_command = "hr"` could never actually take
effect once any other Claude Code tab was open.)

"Team Issues" shows a Linear team's open issues. Unlike a project, a team has no
repo-derived name to match, so it needs an explicit default: set `team_id` in the same
`config.toml`.

```toml
team_id = "linear-team-id"
```

You only need this if your workspace has more than one team — a single-team workspace
resolves automatically. If `team_id` is unset and the workspace has more than one team,
entering "Team Issues" shows an error naming every team (so you can see which id to use)
and pointing at `config.toml`; press `c` on that error screen to open it, same as for the
project-matching errors above.

### Use

Bind keys to the plugin's actions in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+l"
type = "plugin_action"
command = "herdr-linear.open-split"
description = "Open Linear panel"

[[keys.command]]
key = "prefix+shift+l"
type = "plugin_action"
command = "herdr-linear.open-tab"
description = "Open Linear panel (tab)"
```

Reload the config, then press the bound key to open the plugin. The panel opens on a
menu with three options — "My Issues", "Project Issues", and "Team Issues" — all
available. From the menu, use `↑`/`↓` to navigate options,
`Enter` to open the highlighted view, and `q` or `Esc` to quit. Once inside a view,
use `↑`/`↓` to navigate the issue list, `/` to filter it by title or identifier (type to
narrow, `↑`/`↓` still navigate the narrowed list live, `<Enter>` confirms and keeps the
filter applied, `Esc` cancels and restores the full list), `o` to open the selected issue
in your browser, `<Enter>` to implement it (opens a herdr tab, starts the preferred coding
agent, sets the issue to "In Progress", and injects an implement prompt once the agent is
ready), `r` to retry after an error, `c` to open `config.toml` from an error screen (see
"Configure" above — creates the file if it doesn't exist yet), and `Esc` to return to the
menu. Press `q` to quit the
panel from anywhere (menu or view). Pressing the key again focuses the panel if it's
open elsewhere, or closes it if it's already focused.

> [!NOTE]
> `<Enter>` starts the coding agent in the directory herdr reports as your currently
> focused pane/workspace (via its injected launch context), not the plugin process's
> own working directory — so it resolves correctly whether you opened the panel via
> the **split** action (`herdr-linear.open-split`) or the **tab** one
> (`herdr-linear.open-tab`). This requires herdr ≥ 0.7.0 (see `min_herdr_version` in
> `herdr-plugin.toml`); on an older/misbehaving herdr that omits the launch context,
> it falls back to the plugin's own install directory. If that fallback also fails
> (an unreadable process directory), `<Enter>` sets an actionable status instead of
> silently starting the agent nowhere in particular.

To see what the plugin is doing internally (e.g. while debugging a cwd-resolution or
`herdr` CLI issue), set `HERDR_LINEAR_LOG_FILE` to a file path before launching herdr —
the plugin writes its `tracing` diagnostics there instead of to stdout, which would
otherwise corrupt the TUI:

```bash
export HERDR_LINEAR_LOG_FILE=/tmp/herdr-linear.log
```

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
