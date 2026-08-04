# Herdr Plugin Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the existing `herdr-linear` Rust library into an installable Herdr plugin: a `herdr-plugin.toml` manifest plus a ratatui TUI binary that shows the authenticated user's assigned Linear issues in a split pane or a tab, with the same open/focus/close toggle behavior as the reference `herdr-file-viewer` plugin.

**Architecture:** A new binary (replacing the current placeholder `src/main.rs`) lives in the existing crate behind a new `plugin` Cargo feature. It uses the already-tested `LinearClient` to fetch issues, a small ratatui TUI to render them, and a `--launch-decision`/`--launch-decision-tab` argv mode (fed `herdr pane list` JSON on stdin) so idempotent bash launcher scripts can decide whether to open, focus, close, or switch to the panel.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, existing `herdr_linear` library (reqwest/tokio/serde), bash launcher scripts, Herdr 0.7+.

**Spec:** `docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md`

## Global Constraints

- `min_herdr_version = "0.7.0"` in the manifest.
- `platforms = ["linux", "macos"]` — no Windows support in v1.
- New dependencies are gated behind the `plugin` Cargo feature only: `ratatui = "0.30.1"`, `crossterm = "0.29.0"`, `toml = { version = "1.1", default-features = false, features = ["parse", "serde"] }`, `open = "5"`. `tempfile` is a new plain dev-dependency (test-only).
- The default (no-feature) library build must stay free of TUI dependencies — only the `herdr-linear` binary target requires them (`required-features = ["plugin"]`).
- API key resolution order: `$HERDR_PLUGIN_CONFIG_DIR/config.toml` (`api_key` field) first, then the `LINEAR_API_KEY` env var. The resolved key is never logged or printed.
- Pane identification is by `label == "Linear"` — the pane `label` Herdr applies automatically from the manifest's `[[panes]] title` when opened via `plugin pane open --entrypoint linear-panel`. No custom tagging protocol.
- Any pane/tab id returned by the launch-decision logic must be validated "flag-safe" (non-empty, does not start with `-`) before a launcher script passes it to `herdr pane zoom|close` / `herdr tab focus`.
- `[[build]]` in the manifest is a plain `cargo build --release --features plugin` — no prebuilt-binary download/checksum step in v1.
- No automated end-to-end test against a live Herdr instance in v1 — the final task ends with a manual verification checklist instead.
- `just check` (`cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`) must stay green after every task.

---

## Task 1: Scaffold the `plugin` feature and relocate the old demo binary

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs` (add the `plugin` module)
- Modify: `src/main.rs` (full replacement)
- Create: `src/plugin/mod.rs`
- Create: `examples/tracing_demo.rs`
- Modify: `README.md`, `PROJECT_SETUP.md`, `CONTRIBUTING.md` (fix stale `cargo run --bin herdr-linear` references — after this task that binary requires `--features plugin` and is no longer the logging demo)

**Interfaces:**
- Consumes: nothing new.
- Produces: an empty `herdr_linear::plugin` module tree that later tasks add submodules to; the `herdr-linear` binary now requires `--features plugin` to build.

**Note on where `plugin` lives:** the plugin code is a module of the *library* crate (`src/lib.rs`), not of the binary crate (`src/main.rs`) — declared as `pub mod plugin;` so `src/main.rs` (a separate crate that depends on the library, same as any external consumer) uses it via `use herdr_linear::plugin;`. This matters for lint correctness: a `pub` item in a *library* crate is exempt from the `dead_code` lint (the compiler assumes external code might call it), but a `pub` item that lives directly in a *binary* crate is not exempt — it would trip `-D warnings` in every task before Task 10 finally calls everything from `main`. Putting `plugin` in the library from the start avoids that trap.

- [ ] **Step 1: Move the current `src/main.rs` demo into `examples/tracing_demo.rs`**

Read the current `src/main.rs` content, then create `examples/tracing_demo.rs` with that exact content, only updating the header comment's run instructions:

```rust
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
            info!("Found {} teams", teams_conn.total_count);
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
            info!("Found {} issues", issues_conn.total_count);
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
```

- [ ] **Step 2: Replace `src/main.rs` with the plugin binary stub**

```rust
//! herdr-linear plugin binary — a Herdr TUI panel showing the viewer's assigned
//! Linear issues. See docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("herdr-linear plugin scaffold — TUI not implemented yet (see Task 9/10)");
    Ok(())
}
```

- [ ] **Step 3: Create the empty plugin module root**

```rust
//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key resolution), `launch`
//! (open/focus/close/switch decision logic), `app` (TUI state), `ui` (rendering),
//! `data` (Linear data fetching for the plugin).
```

Save as `src/plugin/mod.rs`.

- [ ] **Step 4: Wire the plugin module into the library**

In `src/lib.rs`, add the new module below the existing `pub mod queries;` line:

```rust
#[cfg(feature = "plugin")]
pub mod plugin;
```

Leave the existing `pub mod client;` / `pub mod error;` / `pub mod models;` / `pub mod queries;` and the `pub use ...` lines below them unchanged.

- [ ] **Step 5: Update `Cargo.toml`**

Replace the `[dependencies]`, `[dev-dependencies]`, `[features]`, and `[[bin]]` sections:

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
graphql_client = "0.13"
async-trait = "0.1"
thiserror = "1.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
dotenvy = "0.15"

# Plugin binary only (enabled via the `plugin` feature)
ratatui = { version = "0.30.1", optional = true }
crossterm = { version = "0.29.0", optional = true }
toml = { version = "1.1", default-features = false, features = ["parse", "serde"], optional = true }
open = { version = "5", optional = true }

[dev-dependencies]
tokio-test = "0.4"
mockito = "1.2"
tempfile = "3"

[features]
default = []
plugin = ["ratatui", "crossterm", "toml", "open"]

[[bin]]
name = "herdr-linear"
path = "src/main.rs"
required-features = ["plugin"]
```

This removes the `clap` dependency (declared previously but never actually used by any code) and the old `cli` feature name.

- [ ] **Step 6: Fix stale `cargo run --bin herdr-linear` references in the docs**

That command used to run the logging demo; after this task it requires `--features
plugin` and runs the (still-scaffolded) plugin binary instead. Update each occurrence
to point at the relocated example:

In `README.md`, replace both occurrences of:
```
cargo run --bin herdr-linear
```
with:
```
cargo run --example tracing_demo
```
(one occurrence is standalone on its own line; the other is the line
`RUST_LOG=info cargo run --bin herdr-linear 2>&1 | jq` — replace only the
`cargo run --bin herdr-linear` portion, keeping the rest of that line intact).

In `PROJECT_SETUP.md`, replace:
```
RUST_LOG=info cargo run --bin herdr-linear 2>&1 | jq
```
with:
```
RUST_LOG=info cargo run --example tracing_demo 2>&1 | jq
```

In `CONTRIBUTING.md`, replace:
```
cargo run --bin herdr-linear
```
with:
```
cargo run --example tracing_demo
```

- [ ] **Step 7: Verify the default (library-only) build still works**

Run: `cargo build`
Expected: succeeds, builds only the library (the `herdr-linear` binary is skipped since `plugin` is not enabled by default, and `#[cfg(feature = "plugin")] pub mod plugin;` is compiled out).

- [ ] **Step 8: Verify the plugin binary builds**

Run: `cargo build --features plugin`
Expected: succeeds, prints the scaffold binary at `target/debug/herdr-linear`.

- [ ] **Step 9: Verify the relocated example still compiles**

Run: `cargo build --example tracing_demo`
Expected: succeeds.

- [ ] **Step 10: Run the full quality gate**

Run: `just check`
Expected: fmt clean, clippy clean (`--all-features` now includes `plugin`), all existing tests still pass.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml src/lib.rs src/main.rs src/plugin/mod.rs examples/tracing_demo.rs README.md PROJECT_SETUP.md CONTRIBUTING.md
git commit -m "feat: scaffold plugin binary behind a plugin Cargo feature"
```

---

## Task 2: Resolve the Linear API key for the plugin

**Files:**
- Create: `src/plugin/config.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `herdr_linear::{Error, Result}` (existing library types — `Error::ConfigError(String)` already exists).
- Produces: `plugin::config::resolve_api_key(config_dir: Option<&Path>, env_api_key: Option<&str>) -> Result<String>` (pure, used by later tasks' tests) and `plugin::config::load() -> Result<String>` (reads the real environment, used by `main.rs` in Task 10).

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/config.rs` with the test module only (no implementation yet):

```rust
//! Resolves the Linear API key for the plugin: the plugin's own config file first,
//! falling back to the `LINEAR_API_KEY` environment variable.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_api_key_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_from_file\"\n").unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_file");
    }

    #[test]
    fn falls_back_to_env_var_when_config_file_missing() {
        let dir = tempfile::tempdir().unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_env");
    }

    #[test]
    fn falls_back_to_env_var_when_config_file_has_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "other_field = \"x\"\n").unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_env");
    }

    #[test]
    fn errors_with_resolved_path_when_neither_source_has_a_key() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_api_key(Some(dir.path()), None).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("config.toml"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn errors_when_config_dir_is_unknown_and_no_env_var() {
        let err = resolve_api_key(None, None).unwrap_err();

        assert!(err.to_string().contains("LINEAR_API_KEY"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::config -- --nocapture`
Expected: FAIL to compile — `resolve_api_key` is not defined.

- [ ] **Step 3: Implement `resolve_api_key` and `load`**

Add above the `#[cfg(test)]` block in `src/plugin/config.rs`. Note `crate::` (not
`herdr_linear::`) — this file is compiled as part of the `herdr_linear` library crate
itself (see Task 1's note on why `plugin` lives in the library):

```rust
use crate::{Error, Result};
use std::path::Path;

#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
}

/// Resolve the Linear API key: `config_dir/config.toml`'s `api_key` field first,
/// then `env_api_key`. Pure function — callers own reading the real environment
/// (see [`load`]) so this is deterministic and safe to unit test.
pub fn resolve_api_key(config_dir: Option<&Path>, env_api_key: Option<&str>) -> Result<String> {
    if let Some(dir) = config_dir {
        let config_path = dir.join("config.toml");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(parsed) = toml::from_str::<ConfigFile>(&contents) {
                if let Some(key) = parsed.api_key {
                    if !key.is_empty() {
                        return Ok(key);
                    }
                }
            }
        }
    }

    if let Some(key) = env_api_key {
        if !key.is_empty() {
            return Ok(key.to_string());
        }
    }

    let path_hint = config_dir
        .map(|dir| dir.join("config.toml").display().to_string())
        .unwrap_or_else(|| "<HERDR_PLUGIN_CONFIG_DIR not set>/config.toml".to_string());

    Err(Error::ConfigError(format!(
        "No Linear API key found. Set `api_key` in {path_hint} or export LINEAR_API_KEY."
    )))
}

/// Resolve the Linear API key from the real environment: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`
/// then `$LINEAR_API_KEY`. Thin wrapper around [`resolve_api_key`] used by the binary.
pub fn load() -> Result<String> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    let env_api_key = std::env::var("LINEAR_API_KEY").ok();
    resolve_api_key(config_dir.as_deref(), env_api_key.as_deref())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::config -- --nocapture`
Expected: all 5 tests PASS.

- [ ] **Step 5: Wire the module in and run the full gate**

Add to `src/plugin/mod.rs`:

```rust
pub mod config;
```

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/config.rs src/plugin/mod.rs
git commit -m "feat: resolve Linear API key from plugin config file or env var"
```

---

## Task 3: Split-pane launch-decision logic

**Files:**
- Create: `src/plugin/launch.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: nothing new (only `serde`/`serde_json`, already dependencies).
- Produces: `plugin::launch::launch_decision(pane_list_json: &str) -> String`, returning `"OPEN"`, `"FOCUS <pane_id>"`, or `"CLOSE <pane_id>"`. Also produces the private `Pane`/`PaneListResponse` structs and `is_flag_safe` helper that Task 4 extends in the same file.

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/launch.rs`:

```rust
//! Launch-decision logic: given a herdr `pane list` JSON response, decide whether a
//! launcher script should open a fresh panel, focus an existing one, or close it.
//! Pure and unit-tested — no herdr socket needed. Mirrors the herdr-file-viewer
//! plugin's `launch.rs`, verified against a real herdr 0.7.3 `pane list` response.

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_list_json(panes: &str) -> String {
        format!(r#"{{"id":"cli:pane:list","result":{{"panes":[{panes}],"type":"pane_list"}}}}"#)
    }

    #[test]
    fn opens_when_json_is_unparseable() {
        assert_eq!(launch_decision("not json"), "OPEN");
    }

    #[test]
    fn opens_when_no_pane_is_focused() {
        let json = pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":false}"#);
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn opens_when_no_linear_panel_in_focused_tab() {
        let json = pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":true}"#);
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn closes_when_the_linear_panel_is_focused() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "CLOSE p1");
    }

    #[test]
    fn focuses_when_the_linear_panel_exists_but_is_not_focused() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true},
               {"pane_id":"p2","tab_id":"t1","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "FOCUS p2");
    }

    #[test]
    fn ignores_a_linear_panel_in_a_different_tab() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true},
               {"pane_id":"p2","tab_id":"t2","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "OPEN");
    }

    #[test]
    fn opens_rather_than_emit_an_unsafe_pane_id() {
        let json = pane_list_json(
            r#"{"pane_id":"--rm","tab_id":"t1","focused":true,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision(&json), "OPEN");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::launch -- --nocapture`
Expected: FAIL to compile — `launch_decision` is not defined.

- [ ] **Step 3: Implement the pane model and `launch_decision`**

Add above the `#[cfg(test)]` block:

```rust
use serde::Deserialize;

const PANEL_LABEL: &str = "Linear";

#[derive(Deserialize)]
struct PaneListResponse {
    result: PaneListResult,
}

#[derive(Deserialize)]
struct PaneListResult {
    #[serde(default)]
    panes: Vec<Pane>,
}

#[derive(Deserialize)]
struct Pane {
    pane_id: Option<String>,
    label: Option<String>,
    #[serde(default)]
    focused: bool,
    tab_id: Option<String>,
}

/// A pane/tab id is safe to interpolate into a `herdr pane`/`herdr tab` argv only if
/// it can't be mistaken for a flag by the shell or by herdr's own argument parser.
fn is_flag_safe(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-')
}

/// Decide the split-pane launcher action from a herdr `pane list` JSON response.
/// Returns `"OPEN"`, `"FOCUS <pane_id>"`, or `"CLOSE <pane_id>"`.
pub fn launch_decision(pane_list_json: &str) -> String {
    let Ok(response) = serde_json::from_str::<PaneListResponse>(pane_list_json) else {
        return "OPEN".to_string();
    };
    let panes = &response.result.panes;

    let Some(focused) = panes.iter().find(|p| p.focused) else {
        return "OPEN".to_string();
    };

    let Some(panel) = panes
        .iter()
        .find(|p| p.label.as_deref() == Some(PANEL_LABEL) && p.tab_id == focused.tab_id)
    else {
        return "OPEN".to_string();
    };

    let Some(id) = panel.pane_id.as_deref().filter(|id| is_flag_safe(id)) else {
        return "OPEN".to_string();
    };

    if panel.pane_id == focused.pane_id {
        format!("CLOSE {id}")
    } else {
        format!("FOCUS {id}")
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::launch -- --nocapture`
Expected: all 7 tests PASS.

- [ ] **Step 5: Wire the module in and run the full gate**

Add to `src/plugin/mod.rs`:

```rust
pub mod launch;
```

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/launch.rs src/plugin/mod.rs
git commit -m "feat: add split-pane launch-decision logic for the plugin"
```

---

## Task 4: Tab launch-decision logic

**Files:**
- Modify: `src/plugin/launch.rs`

**Interfaces:**
- Consumes: `Pane`, `PaneListResponse`, `PaneListResult`, `is_flag_safe`, `PANEL_LABEL` (all from Task 3, same file).
- Produces: `plugin::launch::launch_decision_tab(pane_list_json: &str) -> String`, returning `"OPEN"`, `"FOCUS <pane_id>"`, `"CLOSE <pane_id>"`, or `"SWITCHTAB <tab_id>"`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/plugin/launch.rs` (alongside the existing tests):

```rust
    #[test]
    fn tab_opens_when_no_pane_is_focused() {
        let json = pane_list_json(r#"{"pane_id":"p1","tab_id":"t1","focused":false}"#);
        assert_eq!(launch_decision_tab(&json), "OPEN");
    }

    #[test]
    fn tab_closes_when_the_linear_panel_is_focused() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision_tab(&json), "CLOSE p1");
    }

    #[test]
    fn tab_focuses_a_linear_panel_in_the_focused_tab() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","focused":true},
               {"pane_id":"p2","tab_id":"t1","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision_tab(&json), "FOCUS p2");
    }

    #[test]
    fn tab_switches_to_a_linear_panel_in_another_tab_of_the_same_workspace() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","workspace_id":"w1","focused":true},
               {"pane_id":"p2","tab_id":"t2","workspace_id":"w1","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision_tab(&json), "SWITCHTAB t2");
    }

    #[test]
    fn tab_ignores_a_linear_panel_in_a_different_workspace() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","workspace_id":"w1","focused":true},
               {"pane_id":"p2","tab_id":"t2","workspace_id":"w2","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision_tab(&json), "OPEN");
    }

    #[test]
    fn tab_opens_rather_than_emit_an_unsafe_tab_id() {
        let json = pane_list_json(
            r#"{"pane_id":"p1","tab_id":"t1","workspace_id":"w1","focused":true},
               {"pane_id":"p2","tab_id":"--rm","workspace_id":"w1","focused":false,"label":"Linear"}"#,
        );
        assert_eq!(launch_decision_tab(&json), "OPEN");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::launch -- --nocapture`
Expected: FAIL to compile — `launch_decision_tab` is not defined.

- [ ] **Step 3: Add `workspace_id` to `Pane` and implement `launch_decision_tab`**

`launch_decision` (Task 3) never needed the pane's workspace, so `Pane` didn't
declare that field yet (an unused field would have failed Task 3's own `-D warnings`
gate). Add it now that `launch_decision_tab` uses it. Change the `Pane` struct:

```rust
#[derive(Deserialize)]
struct Pane {
    pane_id: Option<String>,
    label: Option<String>,
    #[serde(default)]
    focused: bool,
    tab_id: Option<String>,
    workspace_id: Option<String>,
}
```

Then add below `launch_decision` in `src/plugin/launch.rs`:

```rust
/// Decide the own-tab launcher action from a herdr `pane list` JSON response.
/// Returns `"OPEN"`, `"FOCUS <pane_id>"`, `"CLOSE <pane_id>"`, or `"SWITCHTAB <tab_id>"`.
/// Never switches across workspaces — a panel living in a different workspace is left
/// alone and a fresh one opens in the current tab instead.
pub fn launch_decision_tab(pane_list_json: &str) -> String {
    let Ok(response) = serde_json::from_str::<PaneListResponse>(pane_list_json) else {
        return "OPEN".to_string();
    };
    let panes = &response.result.panes;

    let Some(focused) = panes.iter().find(|p| p.focused) else {
        return "OPEN".to_string();
    };
    let is_panel = |p: &&Pane| p.label.as_deref() == Some(PANEL_LABEL);

    if let Some(here) = panes.iter().find(|p| is_panel(p) && p.tab_id == focused.tab_id) {
        let Some(id) = here.pane_id.as_deref().filter(|id| is_flag_safe(id)) else {
            return "OPEN".to_string();
        };
        return if here.pane_id == focused.pane_id {
            format!("CLOSE {id}")
        } else {
            format!("FOCUS {id}")
        };
    }

    if focused.workspace_id.is_some() {
        if let Some(elsewhere) = panes
            .iter()
            .find(|p| is_panel(p) && p.workspace_id == focused.workspace_id)
        {
            if let Some(tab) = elsewhere.tab_id.as_deref().filter(|t| is_flag_safe(t)) {
                return format!("SWITCHTAB {tab}");
            }
        }
    }

    "OPEN".to_string()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::launch -- --nocapture`
Expected: all 13 tests in the file PASS.

- [ ] **Step 5: Run the full gate**

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/launch.rs
git commit -m "feat: add tab launch-decision logic for the plugin"
```

---

## Task 5: TUI app state and navigation

**Files:**
- Create: `src/plugin/app.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `herdr_linear::Issue` (existing library type).
- Produces: `plugin::app::AppState` enum (`Loading`, `Loaded { issues: Vec<Issue>, selected: usize }`, `Error { message: String }`), `plugin::app::App` struct with `new()`, `set_issues(Vec<Issue>)`, `set_error(String)`, `retry()`, `move_selection_down()`, `move_selection_up()`, `selected_issue() -> Option<&Issue>`. Task 6 extends this same file with key handling; Task 7's `ui.rs` renders `App`/`AppState`; Task 10's `main.rs` drives it.

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/app.rs`:

```rust
//! TUI application state for the plugin: the issue list, selection, and
//! loading/error status. Pure state transitions — no terminal I/O here.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Issue;
    use serde_json::json;

    fn sample_issue(identifier: &str) -> Issue {
        serde_json::from_value(json!({
            "id": format!("issue-{identifier}"),
            "identifier": identifier,
            "title": format!("Title for {identifier}"),
            "description": null,
            "state": {"id": "state-1", "name": "In Progress", "type": "started"},
            "priority": 2,
            "estimate": null,
            "team": {
                "id": "team-1", "key": "ENG", "name": "Engineering",
                "description": null, "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "assignee": null,
            "creator": {
                "id": "user-1", "email": "a@example.com", "name": "Alice",
                "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "startedAt": null,
            "completedAt": null,
            "cycle": null,
            "project": null,
            "labels": [],
            "url": format!("https://linear.app/team/issue/{identifier}")
        }))
        .expect("valid issue payload")
    }

    #[test]
    fn new_app_starts_in_loading_state() {
        let app = App::new();
        assert!(matches!(app.state, AppState::Loading));
    }

    #[test]
    fn set_issues_moves_to_loaded_with_first_item_selected() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        match &app.state {
            AppState::Loaded { issues, selected } => {
                assert_eq!(issues.len(), 2);
                assert_eq!(*selected, 0);
            }
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn move_selection_down_advances_and_clamps_at_the_end() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");

        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn move_selection_up_retreats_and_clamps_at_the_start() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn navigation_on_an_empty_list_does_not_panic() {
        let mut app = App::new();
        app.set_issues(vec![]);

        app.move_selection_down();
        app.move_selection_up();

        assert!(app.selected_issue().is_none());
    }

    #[test]
    fn set_error_moves_to_error_state() {
        let mut app = App::new();
        app.set_error("boom".to_string());

        assert!(matches!(&app.state, AppState::Error { message } if message == "boom"));
    }

    #[test]
    fn retry_moves_back_to_loading() {
        let mut app = App::new();
        app.set_error("boom".to_string());

        app.retry();

        assert!(matches!(app.state, AppState::Loading));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::app -- --nocapture`
Expected: FAIL to compile — `App`/`AppState` are not defined.

- [ ] **Step 3: Implement `AppState` and `App`**

Add above the `#[cfg(test)]` block. Note `crate::Issue` (not `herdr_linear::Issue`) —
this file is compiled as part of the `herdr_linear` library crate itself (see Task 1's
note on why `plugin` lives in the library):

```rust
use crate::Issue;

// Not `PartialEq`: `Issue` (and its nested `Team`/`User`/`IssueState`) don't derive
// it in `src/models.rs`, so tests below use `matches!` instead of `assert_eq!`.
#[derive(Debug, Clone)]
pub enum AppState {
    Loading,
    Loaded { issues: Vec<Issue>, selected: usize },
    Error { message: String },
}

pub struct App {
    pub state: AppState,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::Loading,
        }
    }

    pub fn set_issues(&mut self, issues: Vec<Issue>) {
        self.state = AppState::Loaded {
            issues,
            selected: 0,
        };
    }

    pub fn set_error(&mut self, message: String) {
        self.state = AppState::Error { message };
    }

    pub fn retry(&mut self) {
        self.state = AppState::Loading;
    }

    pub fn move_selection_down(&mut self) {
        if let AppState::Loaded { issues, selected } = &mut self.state {
            if !issues.is_empty() && *selected + 1 < issues.len() {
                *selected += 1;
            }
        }
    }

    pub fn move_selection_up(&mut self) {
        if let AppState::Loaded { selected, .. } = &mut self.state {
            if *selected > 0 {
                *selected -= 1;
            }
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        match &self.state {
            AppState::Loaded { issues, selected } => issues.get(*selected),
            _ => None,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
```

`AppState` needs `#[derive(Debug, ...)]` (already included above) for the `Issue` type it wraps — confirm `Issue` derives `Debug` (it does, per `src/models.rs`: `#[derive(Debug, Clone, Serialize, Deserialize)]`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::app -- --nocapture`
Expected: all 7 tests PASS.

- [ ] **Step 5: Wire the module in and run the full gate**

Add to `src/plugin/mod.rs`:

```rust
pub mod app;
```

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/app.rs src/plugin/mod.rs
git commit -m "feat: add plugin TUI app state and navigation"
```

---

## Task 6: TUI key handling

**Files:**
- Modify: `src/plugin/app.rs`
- Modify: `Cargo.toml` is not needed (crossterm already added in Task 1).

**Interfaces:**
- Consumes: `App`, `AppState` (Task 5, same file).
- Produces: `plugin::app::Action` enum (`Quit`, `OpenInBrowser(String)`, `Retry`) and `plugin::app::handle_key(app: &mut App, key: crossterm::event::KeyCode) -> Option<Action>`. Task 10's event loop calls `handle_key` and matches on `Action`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/plugin/app.rs`:

```rust
    use crossterm::event::KeyCode;

    #[test]
    fn down_key_moves_selection_and_returns_no_action() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Down);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn up_key_moves_selection_and_returns_no_action() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        let action = handle_key(&mut app, KeyCode::Up);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn o_key_returns_open_in_browser_with_the_selected_issue_url() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('o'));

        assert_eq!(
            action,
            Some(Action::OpenInBrowser(
                "https://linear.app/team/issue/ENG-1".to_string()
            ))
        );
    }

    #[test]
    fn o_key_on_an_empty_list_returns_no_action() {
        let mut app = App::new();
        app.set_issues(vec![]);

        assert_eq!(handle_key(&mut app, KeyCode::Char('o')), None);
    }

    #[test]
    fn q_key_returns_quit() {
        let mut app = App::new();

        assert_eq!(handle_key(&mut app, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn esc_key_returns_quit() {
        let mut app = App::new();

        assert_eq!(handle_key(&mut app, KeyCode::Esc), Some(Action::Quit));
    }

    #[test]
    fn r_key_in_error_state_retries_and_returns_retry_action() {
        let mut app = App::new();
        app.set_error("boom".to_string());

        let action = handle_key(&mut app, KeyCode::Char('r'));

        assert_eq!(action, Some(Action::Retry));
        assert!(matches!(app.state, AppState::Loading));
    }

    #[test]
    fn r_key_outside_error_state_does_nothing() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1")]);

        assert_eq!(handle_key(&mut app, KeyCode::Char('r')), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::app -- --nocapture`
Expected: FAIL to compile — `Action`/`handle_key` are not defined.

- [ ] **Step 3: Implement `Action` and `handle_key`**

Add below the `App` impl block in `src/plugin/app.rs` (before the `#[cfg(test)]` module):

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    OpenInBrowser(String),
    Retry,
}

/// Map a key press to an [`Action`], applying any state change (navigation, retry)
/// directly to `app`. Returns `None` when the key had no effect or only changed
/// navigation state in place.
pub fn handle_key(app: &mut App, key: crossterm::event::KeyCode) -> Option<Action> {
    use crossterm::event::KeyCode;

    match key {
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        KeyCode::Down => {
            app.move_selection_down();
            None
        }
        KeyCode::Up => {
            app.move_selection_up();
            None
        }
        KeyCode::Char('o') => app
            .selected_issue()
            .map(|issue| Action::OpenInBrowser(issue.url.clone())),
        KeyCode::Char('r') => {
            if matches!(app.state, AppState::Error { .. }) {
                app.retry();
                Some(Action::Retry)
            } else {
                None
            }
        }
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::app -- --nocapture`
Expected: all 15 tests in the file PASS.

- [ ] **Step 5: Run the full gate**

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/app.rs
git commit -m "feat: add plugin TUI key handling"
```

---

## Task 7: TUI rendering

**Files:**
- Create: `src/plugin/ui.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `App`, `AppState` (Task 5).
- Produces: `plugin::ui::draw(frame: &mut ratatui::Frame, app: &App)`. Task 10's event loop calls `terminal.draw(|frame| plugin::ui::draw(frame, &app))`.

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/ui.rs`:

```rust
//! Rendering for the plugin TUI: a loading message, an error message with a retry
//! hint, or a two-pane issue list + detail view.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::app::App;
    use crate::Issue;
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;

    fn sample_issue(identifier: &str) -> Issue {
        serde_json::from_value(json!({
            "id": format!("issue-{identifier}"),
            "identifier": identifier,
            "title": format!("Title for {identifier}"),
            "description": null,
            "state": {"id": "state-1", "name": "In Progress", "type": "started"},
            "priority": 2,
            "estimate": null,
            "team": {
                "id": "team-1", "key": "ENG", "name": "Engineering",
                "description": null, "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "assignee": null,
            "creator": {
                "id": "user-1", "email": "a@example.com", "name": "Alice",
                "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "startedAt": null,
            "completedAt": null,
            "cycle": null,
            "project": null,
            "labels": [],
            "url": format!("https://linear.app/team/issue/{identifier}")
        }))
        .expect("valid issue payload")
    }

    fn rendered_text(app: &App) -> String {
        let backend = TestBackend::new(60, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_loading_message() {
        let app = App::new();
        assert!(rendered_text(&app).contains("Loading"));
    }

    #[test]
    fn renders_error_message_with_retry_hint() {
        let mut app = App::new();
        app.set_error("Authentication failed".to_string());

        let text = rendered_text(&app);
        assert!(text.contains("Authentication failed"));
        assert!(text.contains("retry"));
    }

    #[test]
    fn renders_issue_identifier_and_title_in_the_list() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1"));
        assert!(text.contains("Title for ENG-1"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin plugin::ui -- --nocapture`
Expected: FAIL to compile — `draw` is not defined.

- [ ] **Step 3: Implement `draw`**

Add above the `#[cfg(test)]` block in `src/plugin/ui.rs`:

```rust
use crate::plugin::app::{App, AppState};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &App) {
    match &app.state {
        AppState::Loading => {
            let paragraph = Paragraph::new("Loading issues...")
                .block(Block::default().borders(Borders::ALL).title("Linear"));
            frame.render_widget(paragraph, frame.area());
        }
        AppState::Error { message } => {
            let paragraph = Paragraph::new(format!("{message}\n\nPress r to retry."))
                .block(Block::default().borders(Borders::ALL).title("Linear - Error"));
            frame.render_widget(paragraph, frame.area());
        }
        AppState::Loaded { issues, selected } => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(frame.area());

            let items: Vec<ListItem> = issues
                .iter()
                .enumerate()
                .map(|(i, issue)| {
                    let text = format!("{} {}", issue.identifier, issue.title);
                    let style = if i == *selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    ListItem::new(Line::from(Span::styled(text, style)))
                })
                .collect();
            let list =
                List::new(items).block(Block::default().borders(Borders::ALL).title("My Issues"));
            frame.render_widget(list, chunks[0]);

            let detail = issues
                .get(*selected)
                .map(|issue| {
                    format!(
                        "{}\n\n{}\n\nState: {}\nURL: {}",
                        issue.identifier, issue.title, issue.state.name, issue.url
                    )
                })
                .unwrap_or_default();
            let detail_widget =
                Paragraph::new(detail).block(Block::default().borders(Borders::ALL).title("Detail"));
            frame.render_widget(detail_widget, chunks[1]);
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin plugin::ui -- --nocapture`
Expected: all 3 tests PASS.

- [ ] **Step 5: Wire the module in and run the full gate**

Add to `src/plugin/mod.rs`:

```rust
pub mod ui;
```

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/ui.rs src/plugin/mod.rs
git commit -m "feat: add plugin TUI rendering"
```

---

## Task 8: Fetch the viewer's assigned issues

**Files:**
- Create: `src/plugin/data.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `herdr_linear::{LinearClient, Issue, Result}` (existing library).
- Produces: `plugin::data::assignee_filter(user_id: &str) -> serde_json::Value` and `plugin::data::fetch_my_issues(client: &LinearClient) -> Result<Vec<Issue>>`. Task 10 calls `fetch_my_issues` from the event loop.

- [ ] **Step 1: Write the failing test**

Create `src/plugin/data.rs`:

```rust
//! Composes existing `LinearClient` calls into what the plugin needs: the
//! authenticated viewer's assigned issues.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_filter_matches_on_the_given_user_id() {
        let filter = assignee_filter("user-123");

        assert_eq!(filter["assignee"]["id"]["eq"], "user-123");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --features plugin plugin::data -- --nocapture`
Expected: FAIL to compile — `assignee_filter` is not defined.

- [ ] **Step 3: Implement `assignee_filter` and `fetch_my_issues`**

Add above the `#[cfg(test)]` block in `src/plugin/data.rs`. Note `crate::` (not
`herdr_linear::`) — this file is compiled as part of the `herdr_linear` library crate
itself (see Task 1's note on why `plugin` lives in the library):

```rust
use crate::{Issue, LinearClient, Result};
use serde_json::{json, Value};

/// A Linear issue filter matching issues assigned to `user_id`.
pub fn assignee_filter(user_id: &str) -> Value {
    json!({ "assignee": { "id": { "eq": user_id } } })
}

/// Fetch the issues assigned to the currently authenticated user.
///
/// `LinearClient` has no dedicated "my issues" call, so this composes
/// `get_viewer()` (to find the current user id) with `get_issues()` filtered
/// to that id as assignee. Both underlying calls are already covered by
/// `LinearClient`'s own tests; this function is thin composition on top.
pub async fn fetch_my_issues(client: &LinearClient) -> Result<Vec<Issue>> {
    let viewer = client.get_viewer().await?;
    let connection = client
        .get_issues(Some(assignee_filter(&viewer.id)), Some(50), None)
        .await?;
    Ok(connection.nodes)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --features plugin plugin::data -- --nocapture`
Expected: the test PASSES.

- [ ] **Step 5: Wire the module in and run the full gate**

Add to `src/plugin/mod.rs`:

```rust
pub mod data;
```

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/data.rs src/plugin/mod.rs
git commit -m "feat: fetch the viewer's assigned issues for the plugin"
```

---

## Task 9: Wire `--launch-decision` / `--launch-decision-tab` dispatch

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `plugin::launch::{launch_decision, launch_decision_tab}` (Tasks 3-4).
- Produces: a private `dispatch_launch_decision(args: &[String], stdin_content: &str) -> Option<String>` function in `src/main.rs`, and updates `main()` to use it. Task 10 replaces the "normal run" branch (currently a placeholder) with the real TUI startup.

- [ ] **Step 1: Write the failing tests**

Add to `src/main.rs`, below the existing `main` function:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_split_launch_decision() {
        let args = vec!["--launch-decision".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn dispatches_tab_launch_decision() {
        let args = vec!["--launch-decision-tab".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn returns_none_for_a_normal_run() {
        assert_eq!(dispatch_launch_decision(&[], ""), None);
    }

    #[test]
    fn returns_none_for_an_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert_eq!(dispatch_launch_decision(&args, ""), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --features plugin --bin herdr-linear -- --nocapture`
Expected: FAIL to compile — `dispatch_launch_decision` is not defined.

- [ ] **Step 3: Implement `dispatch_launch_decision` and wire it into `main`**

Replace the whole `src/main.rs` body (keeping the module doc comment from Task 1) with.
Note `use herdr_linear::plugin;` — `src/main.rs` is a separate (binary) crate from
`herdr_linear`, so it reaches the plugin module the same way any consumer of the
library would, not via a local `mod plugin;` declaration:

```rust
//! herdr-linear plugin binary — a Herdr TUI panel showing the viewer's assigned
//! Linear issues. See docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md.

use herdr_linear::plugin;
use std::io::Read;

/// Dispatch `--launch-decision` / `--launch-decision-tab` to the pure decision
/// functions, reading the `pane list` JSON from `stdin_content`. Returns `None`
/// for a normal run (start the TUI) or an unrecognized flag.
fn dispatch_launch_decision(args: &[String], stdin_content: &str) -> Option<String> {
    match args.first().map(String::as_str) {
        Some("--launch-decision") => Some(plugin::launch::launch_decision(stdin_content)),
        Some("--launch-decision-tab") => Some(plugin::launch::launch_decision_tab(stdin_content)),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if !args.is_empty() {
        let mut stdin_content = String::new();
        std::io::stdin().read_to_string(&mut stdin_content)?;
        if let Some(decision) = dispatch_launch_decision(&args, &stdin_content) {
            println!("{decision}");
            return Ok(());
        }
    }

    println!("herdr-linear plugin scaffold — TUI not implemented yet (see Task 10)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_split_launch_decision() {
        let args = vec!["--launch-decision".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn dispatches_tab_launch_decision() {
        let args = vec!["--launch-decision-tab".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn returns_none_for_a_normal_run() {
        assert_eq!(dispatch_launch_decision(&[], ""), None);
    }

    #[test]
    fn returns_none_for_an_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert_eq!(dispatch_launch_decision(&args, ""), None);
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin --bin herdr-linear -- --nocapture`
Expected: all 4 tests PASS.

- [ ] **Step 5: Run the full gate**

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire launch-decision dispatch into the plugin binary"
```

---

## Task 10: Full TUI wiring, manifest, launcher scripts, README

**Files:**
- Modify: `src/main.rs`
- Create: `herdr-plugin.toml`
- Create: `scripts/open-split.sh`
- Create: `scripts/open-tab.sh`
- Modify: `README.md`

**Interfaces:**
- Consumes: `plugin::config::load()` (Task 2), `plugin::data::fetch_my_issues()` (Task 8), `plugin::app::{App, Action, handle_key}` (Tasks 5-6), `plugin::ui::draw()` (Task 7), `herdr_linear::LinearClient` (existing library).
- Produces: a fully working plugin binary; no new interfaces consumed by later tasks (this is the last task).

- [ ] **Step 1: Replace the placeholder run branch in `src/main.rs` with the real TUI**

Replace the `main` function's final `println!(...)` placeholder line and add the supporting functions. The full file becomes:

Note `use herdr_linear::plugin;` — `src/main.rs` is a separate (binary) crate from
`herdr_linear`, so it reaches the plugin module the same way any consumer of the
library would, not via a local `mod plugin;` declaration:

```rust
//! herdr-linear plugin binary — a Herdr TUI panel showing the viewer's assigned
//! Linear issues. See docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md.

use herdr_linear::plugin;
use std::io::Read;

/// Dispatch `--launch-decision` / `--launch-decision-tab` to the pure decision
/// functions, reading the `pane list` JSON from `stdin_content`. Returns `None`
/// for a normal run (start the TUI) or an unrecognized flag.
fn dispatch_launch_decision(args: &[String], stdin_content: &str) -> Option<String> {
    match args.first().map(String::as_str) {
        Some("--launch-decision") => Some(plugin::launch::launch_decision(stdin_content)),
        Some("--launch-decision-tab") => Some(plugin::launch::launch_decision_tab(stdin_content)),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if !args.is_empty() {
        let mut stdin_content = String::new();
        std::io::stdin().read_to_string(&mut stdin_content)?;
        if let Some(decision) = dispatch_launch_decision(&args, &stdin_content) {
            println!("{decision}");
            return Ok(());
        }
    }

    run_tui().await
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        original_hook(panic_info);
    }));
}

async fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = plugin::config::load()?;
    let client = herdr_linear::LinearClient::new(api_key)?;

    install_panic_hook();

    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = plugin::app::App::new();
    let result = event_loop(&mut terminal, &mut app, &client).await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

async fn load_issues(app: &mut plugin::app::App, client: &herdr_linear::LinearClient) {
    match plugin::data::fetch_my_issues(client).await {
        Ok(issues) => app.set_issues(issues),
        Err(err) => app.set_error(err.to_string()),
    }
}

async fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
) -> Result<(), Box<dyn std::error::Error>> {
    load_issues(app, client).await;

    loop {
        terminal.draw(|frame| plugin::ui::draw(frame, app))?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if let Some(action) = plugin::app::handle_key(app, key.code) {
                    match action {
                        plugin::app::Action::Quit => break,
                        plugin::app::Action::OpenInBrowser(url) => {
                            let _ = open::that(url);
                        }
                        plugin::app::Action::Retry => {
                            load_issues(app, client).await;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_split_launch_decision() {
        let args = vec!["--launch-decision".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn dispatches_tab_launch_decision() {
        let args = vec!["--launch-decision-tab".to_string()];
        assert_eq!(
            dispatch_launch_decision(&args, "not valid json"),
            Some("OPEN".to_string())
        );
    }

    #[test]
    fn returns_none_for_a_normal_run() {
        assert_eq!(dispatch_launch_decision(&[], ""), None);
    }

    #[test]
    fn returns_none_for_an_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert_eq!(dispatch_launch_decision(&args, ""), None);
    }
}
```

- [ ] **Step 2: Verify it builds and the existing dispatch tests still pass**

Run: `cargo test --features plugin --bin herdr-linear -- --nocapture`
Expected: all 4 tests PASS (unchanged from Task 9 — this step only adds the TUI runtime path, which the tests don't exercise directly).

- [ ] **Step 3: Create the plugin manifest**

Create `herdr-plugin.toml` at the repo root:

```toml
# herdr-plugin.toml — manifest for the herdr-linear plugin.
#
# A read-only "My Issues" panel for Linear: lists the authenticated user's assigned
# issues in a herdr split pane or tab, with view details / open-in-browser. See
# docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md for the full design.
#
# v1 targets linux/macos only. The [[build]] step is a plain `cargo build`, so
# installing requires a Rust toolchain (no prebuilt-binary download yet).

id = "herdr-linear"
name = "herdr-linear"
version = "0.1.0"
description = "A read-only Linear issues panel for Herdr."
min_herdr_version = "0.7.0"
platforms = ["linux", "macos"]

[[build]]
command = ["cargo", "build", "--release", "--features", "plugin"]

# The Linear panel. herdr performs the split/tab placement on open; the [[actions]]
# below summon it via the idempotent launcher scripts.
[[panes]]
id = "linear-panel"
title = "Linear"
placement = "split"
command = ["./target/release/herdr-linear"]

[[actions]]
id = "open-split"
title = "Open Linear panel"
description = "Open the Linear issues panel in a split pane beside the current work."
command = ["bash", "scripts/open-split.sh"]

[[actions]]
id = "open-tab"
title = "Open Linear panel (tab)"
description = "Open the Linear issues panel in its own tab (switch to it if already open)."
command = ["bash", "scripts/open-tab.sh"]
```

- [ ] **Step 4: Create the split-pane launcher script**

Create `scripts/open-split.sh`:

```bash
#!/usr/bin/env bash
# Idempotent launcher for the Linear panel split pane. "Launch-or-focus, toggle on
# repeat", scoped to the current tab — mirrors the herdr-file-viewer plugin's
# open-file-viewer.sh:
#   - no Linear pane in the current tab      -> open a split (focused)
#   - a Linear pane exists but isn't focused -> focus it
#   - the focused pane IS the Linear pane    -> close it (toggle off)
#
# The OPEN/FOCUS/CLOSE decision is computed in-process by the plugin binary itself
# (`herdr-linear --launch-decision`, fed `pane list` JSON on stdin) so it is unit
# tested and the pane id it returns is already validated as flag-safe.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
plugin_bin="$script_dir/../target/release/herdr-linear"

open_pane() {
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-linear \
    --entrypoint linear-panel \
    --placement split \
    --direction right \
    --focus
}

decision="OPEN"
if [ -x "$plugin_bin" ]; then
  panes="$("$herdr_bin" pane list 2>/dev/null || true)"
  if [ -n "$panes" ]; then
    decision="$(printf '%s' "$panes" | "$plugin_bin" --launch-decision 2>/dev/null || echo OPEN)"
  fi
fi

case "$decision" in
  "FOCUS "*)
    pid="${decision#FOCUS }"
    "$herdr_bin" pane zoom "$pid" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$pid" --off
    ;;
  "CLOSE "*)
    pid="${decision#CLOSE }"
    exec "$herdr_bin" pane close "$pid"
    ;;
  *)
    open_pane
    ;;
esac
```

- [ ] **Step 5: Create the own-tab launcher script**

Create `scripts/open-tab.sh`:

```bash
#!/usr/bin/env bash
# Idempotent launcher for the Linear panel in its own TAB. "Open-or-switch, toggle
# on repeat", scoped across the tabs of the CURRENT WORKSPACE — mirrors the
# herdr-file-viewer plugin's open-file-viewer-tab.sh. A panel open in a different
# workspace is left alone; a fresh one opens here.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
plugin_bin="$script_dir/../target/release/herdr-linear"

open_tab() {
  exec "$herdr_bin" plugin pane open \
    --plugin herdr-linear \
    --entrypoint linear-panel \
    --placement tab \
    --focus
}

decision="OPEN"
if [ -x "$plugin_bin" ]; then
  panes="$("$herdr_bin" pane list 2>/dev/null || true)"
  if [ -n "$panes" ]; then
    decision="$(printf '%s' "$panes" | "$plugin_bin" --launch-decision-tab 2>/dev/null || echo OPEN)"
  fi
fi

case "$decision" in
  "SWITCHTAB "*)
    tid="${decision#SWITCHTAB }"
    "$herdr_bin" tab focus "$tid" || open_tab
    ;;
  "FOCUS "*)
    pid="${decision#FOCUS }"
    "$herdr_bin" pane zoom "$pid" --on >/dev/null 2>&1 || true
    exec "$herdr_bin" pane zoom "$pid" --off
    ;;
  "CLOSE "*)
    pid="${decision#CLOSE }"
    exec "$herdr_bin" pane close "$pid"
    ;;
  *)
    open_tab
    ;;
esac
```

- [ ] **Step 6: Make the launcher scripts executable**

Run: `chmod +x scripts/open-split.sh scripts/open-tab.sh`

- [ ] **Step 7: Add plugin installation/usage instructions to `README.md`**

Add a new section to `README.md`, after the existing "## Issues & Feature Requests" section:

```markdown
## Herdr Plugin

`herdr-linear` also ships as a [Herdr](https://herdr.dev) plugin: a read-only "My
Issues" panel you can open as a split pane or a tab from inside a Herdr session.

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

Reload the config, then press the bound key: `↑`/`↓` to navigate issues, `o` to
open the selected issue in your browser, `r` to retry after an error, `q`/`Esc` to
quit. Pressing the key again focuses the panel if it's open elsewhere, or closes it
if it's already focused.
```

- [ ] **Step 8: Run the full quality gate**

Run: `just check`
Expected: fmt clean, clippy clean, all tests pass (this task adds no new automated tests — see the manual checklist below).

- [ ] **Step 9: Manual verification checklist**

This step has no automated test (per the design's stated v1 scope — no live-Herdr E2E). From inside a running Herdr session, in the `herdr-linear` repo root:

1. `cargo build --release --features plugin` (the same command the manifest's `[[build]]` runs; builds the release binary the manifest's `[[panes]] command` points at — note plain `just build` does *not* pass `--features plugin` and would not produce this binary).
2. `herdr plugin link .`
3. Set an API key: `mkdir -p "$(herdr plugin config-dir herdr-linear)" && echo 'api_key = "lin_api_..."' > "$(herdr plugin config-dir herdr-linear)/config.toml"`.
4. Add the two `[[keys.command]]` entries from the README to `~/.config/herdr/config.toml`, then `herdr server reload-config`.
5. Press the `open-split` key: a split pane opens showing "Loading issues..." then the issue list.
6. Press it again while the pane is not focused: it focuses instead of duplicating.
7. Press it again while the pane IS focused: it closes.
8. Press the `open-tab` key: opens in a new tab; repeat presses focus/close it; opening a `open-split` split pane in a *different* tab and then pressing `open-tab` switches to that tab instead of opening a duplicate.
9. In the panel, press `↓`/`↑` to move the selection, `o` on an issue to confirm it opens in the system browser.
10. Temporarily rename the config file's `api_key` to something invalid and retry a fresh `OPEN` — confirm the panel shows an inline auth-error message with a retry hint instead of crashing.

- [ ] **Step 10: Commit**

```bash
git add src/main.rs herdr-plugin.toml scripts/open-split.sh scripts/open-tab.sh README.md
git commit -m "feat: wire up the full herdr-linear plugin (TUI loop, manifest, launcher scripts)"
```
