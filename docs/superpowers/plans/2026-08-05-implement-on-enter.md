# Implement-on-Enter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pressing `<Enter>` on a selected issue in a loaded issue-list view opens a new herdr tab titled after the issue, sets the issue to its team's "In Progress" workflow state via a real GraphQL mutation, starts the user's preferred coding agent in that tab, and — once the agent reports ready — injects `Implement Linear Issue <identifier> using a new git worktree`.

**Architecture:** Pure decision logic (agent derivation, command resolution, shell-argv/prompt building, workflow-state picking) lives in a new `plugin::implement` module, fully unit-tested. A new `plugin::herdr_cli` module is a thin, deliberately untested subprocess wrapper around the `herdr` CLI's JSON socket protocol (same status as the existing `open::that(url)` call). `app.rs` gains `Action::Implement(Issue)`, `<Enter>` handling, and a transient status banner separate from the existing hard error screen. `main.rs` orchestrates the sequential flow, calling into both new modules and the existing `LinearClient`.

**Tech Stack:** Rust, tokio (async + `tokio::process::Command`), ratatui/crossterm (TUI), serde/serde_json, the `herdr` CLI (external binary, JSON-over-stdout socket protocol).

## Global Constraints

- Shell-wrap every resolved agent command uniformly: `argv = [$SHELL or "/bin/bash", "-i", "-c", "<command>"]` — both a bare derived binary (`claude`) and a config alias/function (`hr`) must resolve.
- Preferred-agent precedence: derived from other open herdr tabs → `agent_command` in `config.toml` → `"hr"` default.
- Readiness signal: `herdr agent wait <pane_id> --status idle --timeout 30000`. No pane-output pattern matching.
- No rollback on partial failure. Every step's failure sets a specific, actionable status message on `App` and either continues (tab rename, Linear state update) or stops (agent list, agent start, agent wait, agent send) per the design doc's data flow — never silent, never a panic.
- Reuse `Error::Internal` for all `herdr_cli` failures — no new `Error` variant.
- The literal injected prompt is exactly: `Implement Linear Issue <identifier> using a new git worktree` — no other wording.
- `herdr` binary resolution: `$HERDR_BIN_PATH` env var, falling back to `"herdr"` on `$PATH` — same convention as `scripts/open-tab.sh`.
- Full spec: `docs/superpowers/specs/2026-08-05-implement-on-enter-design.md`.

---

### Task 1: `agent_command` config override

**Files:**
- Modify: `src/plugin/config.rs`

**Interfaces:**
- Produces: `pub fn resolve_agent_command_override(config_dir: Option<&Path>) -> Result<Option<String>>`, `pub fn load_agent_command_override() -> Result<Option<String>>`, both consumed by Task 6's `main.rs` orchestration.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/config.rs`'s existing `#[cfg(test)] mod tests` block (inside the `use super::{resolve_api_key, resolve_project_id_override};` — extend that import too):

```rust
    use super::{resolve_agent_command_override, resolve_api_key, resolve_project_id_override};
```

Then add these test functions (anywhere inside `mod tests`, alongside the existing `project_id` tests):

```rust
    #[test]
    fn reads_agent_command_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "agent_command = \"claude\"\n",
        )
        .unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, Some("claude".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_agent_command() {
        let dir = tempfile::tempdir().unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn returns_none_when_agent_command_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "agent_command = \"   \"\n").unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_agent_command() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_agent_command() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_agent_command_override(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --all-features --lib plugin::config -- --nocapture`
Expected: compile error — `resolve_agent_command_override` and the `agent_command` TOML field don't exist yet.

- [ ] **Step 3: Implement**

In `src/plugin/config.rs`, add the `agent_command` field to `ConfigFile`:

```rust
#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
    project_id: Option<String>,
    agent_command: Option<String>,
}
```

Add these two functions after `resolve_project_id_override` / `load_project_id_override`:

```rust
/// Resolve an `agent_command` override: `config_dir/config.toml`'s `agent_command` field, if
/// set and non-empty. `Ok(None)` means "no override" (callers fall back to the agent name
/// derived from other open herdr tabs, then finally `"hr"` — see
/// [`crate::plugin::implement::resolve_agent_command`]). Pure function — callers own reading
/// the real environment (see [`load_agent_command_override`]).
pub fn resolve_agent_command_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let agent_command = read_config_file(config_dir)?
        .and_then(|file| file.agent_command)
        .filter(|cmd| !cmd.trim().is_empty());
    Ok(agent_command)
}

/// Resolve the `agent_command` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_agent_command_override`]; called from `main.rs`'s `start_implementation`.
pub fn load_agent_command_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_agent_command_override(config_dir.as_deref())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::config -- --nocapture`
Expected: PASS — all `agent_command` tests plus the pre-existing `api_key`/`project_id` tests in the same module.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/config.rs
git commit -m "feat: add agent_command config override (TF-584)"
```

---

### Task 2: `plugin::implement` — pure decision logic

**Files:**
- Create: `src/plugin/implement.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `crate::IssueState` (existing, `models.rs`).
- Produces (all consumed by Task 6's `main.rs` orchestration):
  - `pub fn resolve_preferred_agent(agent_list_json: &str) -> Option<String>`
  - `pub fn resolve_agent_command(derived: Option<&str>, config_override: Option<&str>) -> String`
  - `pub fn build_shell_argv(shell: &str, command: &str) -> Vec<String>`
  - `pub fn build_implement_prompt(identifier: &str) -> String`
  - `pub fn pick_in_progress_state<'a>(states: &'a [IssueState]) -> Option<&'a IssueState>`

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/implement.rs` with only the doc comment, imports, and test module (implementation comes in Step 3):

```rust
//! Pure decision logic for the "implement this issue" flow triggered by `<Enter>` in an
//! issue list: deriving the preferred coding agent from other open herdr tabs, resolving
//! the final agent command, building the shell-wrapped argv to launch it, building the
//! literal prompt injected once the agent is ready, and picking the right workflow state to
//! move the issue to. No process/socket access here — see [`crate::plugin::herdr_cli`] for
//! that; this module only ever sees JSON text and in-memory values.

use crate::IssueState;
use serde::Deserialize;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: &str, name: &str, r#type: &str) -> IssueState {
        IssueState {
            id: id.to_string(),
            name: name.to_string(),
            r#type: r#type.to_string(),
        }
    }

    #[test]
    fn resolve_preferred_agent_returns_the_most_frequent_agent() {
        let json = r#"{"agents":[{"agent":"claude"},{"agent":"claude"},{"agent":"codex"}]}"#;

        assert_eq!(resolve_preferred_agent(json), Some("claude".to_string()));
    }

    #[test]
    fn resolve_preferred_agent_breaks_ties_by_first_seen_order() {
        let json = r#"{"agents":[{"agent":"codex"},{"agent":"claude"}]}"#;

        assert_eq!(resolve_preferred_agent(json), Some("codex".to_string()));
    }

    #[test]
    fn resolve_preferred_agent_skips_panes_without_an_agent_field() {
        let json = r#"{"agents":[{"agent_status":"unknown"},{"agent":"claude"}]}"#;

        assert_eq!(resolve_preferred_agent(json), Some("claude".to_string()));
    }

    #[test]
    fn resolve_preferred_agent_returns_none_for_an_empty_list() {
        assert_eq!(resolve_preferred_agent(r#"{"agents":[]}"#), None);
    }

    #[test]
    fn resolve_preferred_agent_returns_none_when_every_entry_has_no_agent() {
        let json = r#"{"agents":[{"agent_status":"unknown"},{"agent_status":"idle"}]}"#;

        assert_eq!(resolve_preferred_agent(json), None);
    }

    #[test]
    fn resolve_preferred_agent_returns_none_for_malformed_json() {
        assert_eq!(resolve_preferred_agent("not json"), None);
    }

    #[test]
    fn resolve_agent_command_prefers_the_derived_agent() {
        assert_eq!(
            resolve_agent_command(Some("claude"), Some("hr")),
            "claude"
        );
    }

    #[test]
    fn resolve_agent_command_falls_back_to_the_config_override() {
        assert_eq!(resolve_agent_command(None, Some("hr")), "hr");
    }

    #[test]
    fn resolve_agent_command_falls_back_to_hr_by_default() {
        assert_eq!(resolve_agent_command(None, None), "hr");
    }

    #[test]
    fn build_shell_argv_wraps_the_command_through_an_interactive_shell() {
        assert_eq!(
            build_shell_argv("/bin/zsh", "hr"),
            vec![
                "/bin/zsh".to_string(),
                "-i".to_string(),
                "-c".to_string(),
                "hr".to_string()
            ]
        );
    }

    #[test]
    fn build_implement_prompt_matches_the_exact_wording() {
        assert_eq!(
            build_implement_prompt("TF-563"),
            "Implement Linear Issue TF-563 using a new git worktree"
        );
    }

    #[test]
    fn pick_in_progress_state_matches_by_name_case_insensitively() {
        let states = vec![
            state("s1", "Backlog", "backlog"),
            state("s2", "in progress", "started"),
            state("s3", "In Review", "started"),
        ];

        let picked = pick_in_progress_state(&states).unwrap();

        assert_eq!(picked.id, "s2");
    }

    #[test]
    fn pick_in_progress_state_falls_back_to_the_first_started_type() {
        let states = vec![
            state("s1", "Backlog", "backlog"),
            state("s2", "In Review", "started"),
            state("s3", "Done", "completed"),
        ];

        let picked = pick_in_progress_state(&states).unwrap();

        assert_eq!(picked.id, "s2");
    }

    #[test]
    fn pick_in_progress_state_returns_none_when_no_state_matches() {
        let states = vec![state("s1", "Backlog", "backlog"), state("s2", "Done", "completed")];

        assert_eq!(pick_in_progress_state(&states), None);
    }

    #[test]
    fn pick_in_progress_state_returns_none_for_an_empty_list() {
        assert_eq!(pick_in_progress_state(&[]), None);
    }
}
```

- [ ] **Step 2: Register the module and verify the tests fail to compile**

In `src/plugin/mod.rs`, add `pub mod implement;` (alphabetically, after `data`):

```rust
pub mod app;
pub mod config;
pub mod data;
pub mod implement;
pub mod launch;
pub mod repo;
pub mod ui;
```

Also update the module doc comment at the top of `mod.rs` to mention it — append after the `repo` mention:

```rust
//! Submodules are added incrementally: `config` (API key / project-id / agent-command
//! resolution), `launch` (open/focus/close/switch decision logic), `app` (TUI state), `ui`
//! (rendering), `data` (Linear data fetching for the plugin), `repo` (CWD → Linear project
//! resolution), `implement` (implement-on-Enter decision logic), `herdr_cli` (herdr CLI
//! subprocess wrapper).
```

Run: `cargo test --all-features --lib plugin::implement -- --nocapture`
Expected: compile error — none of the five functions exist yet.

- [ ] **Step 3: Implement**

Insert the following above the `#[cfg(test)]` line in `src/plugin/implement.rs`:

```rust
#[derive(Deserialize)]
struct AgentListResult {
    agents: Vec<AgentEntry>,
}

#[derive(Deserialize)]
struct AgentEntry {
    agent: Option<String>,
}

/// Derive the preferred coding agent name from a herdr `agent list` JSON result (the
/// already-unwrapped `result` value — see [`crate::plugin::herdr_cli::agent_list`]): the most
/// frequent non-null `agent` value across all reported agent panes, ties broken by first-seen
/// order. Returns `None` on unparseable JSON, an empty agent list, or when every entry's
/// `agent` is null/absent — all of which fall through to [`resolve_agent_command`]'s
/// config/default path.
pub fn resolve_preferred_agent(agent_list_json: &str) -> Option<String> {
    let parsed: AgentListResult = serde_json::from_str(agent_list_json).ok()?;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for entry in &parsed.agents {
        let Some(agent) = entry.agent.as_deref() else {
            continue;
        };
        *counts.entry(agent).or_insert(0) += 1;
        if !order.contains(&agent) {
            order.push(agent);
        }
    }

    // `max_by_key` returns the *last* element among ties, so iterate in reverse to make the
    // *first*-seen agent win ties (see the `_breaks_ties_by_first_seen_order` test above).
    order
        .iter()
        .rev()
        .max_by_key(|agent| counts[*agent])
        .map(|s| s.to_string())
}

/// Resolve the final agent command: `derived` (from other open tabs) wins, then
/// `config_override` (`agent_command` in `config.toml`), then the `"hr"` default.
pub fn resolve_agent_command(derived: Option<&str>, config_override: Option<&str>) -> String {
    derived.or(config_override).unwrap_or("hr").to_string()
}

/// Build the argv to run `command` through an interactive instance of `shell`, so both a bare
/// binary name (e.g. `"claude"`) and a shell alias/function defined in an rc file (e.g.
/// `"hr"`) resolve correctly.
pub fn build_shell_argv(shell: &str, command: &str) -> Vec<String> {
    vec![
        shell.to_string(),
        "-i".to_string(),
        "-c".to_string(),
        command.to_string(),
    ]
}

/// Build the literal prompt injected into the agent's pane once it's ready.
pub fn build_implement_prompt(identifier: &str) -> String {
    format!("Implement Linear Issue {identifier} using a new git worktree")
}

/// Pick the workflow state to move an issue to when starting implementation: a
/// case-insensitive name match on `"In Progress"` first, else the first `type == "started"`
/// state, else `None`.
pub fn pick_in_progress_state(states: &[IssueState]) -> Option<&IssueState> {
    states
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case("in progress"))
        .or_else(|| states.iter().find(|s| s.r#type == "started"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::implement -- --nocapture`
Expected: PASS — all 15 tests.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/implement.rs src/plugin/mod.rs
git commit -m "feat: add plugin::implement pure decision logic (TF-584)"
```

---

### Task 3: `plugin::herdr_cli` — herdr CLI subprocess wrapper

**Files:**
- Create: `src/plugin/herdr_cli.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: `crate::error::Error`, `crate::Result`.
- Produces (all consumed by Task 6's `main.rs` orchestration; no automated tests — see Global Constraints and Task 6's manual verification):
  - `pub fn herdr_bin() -> String`
  - `pub async fn agent_list(herdr_bin: &str) -> Result<String>`
  - `pub async fn agent_start(herdr_bin: &str, name: &str, cwd: &Path, argv: &[String]) -> Result<AgentStarted>` where `pub struct AgentStarted { pub pane_id: String, pub tab_id: String }`
  - `pub async fn tab_rename(herdr_bin: &str, tab_id: &str, label: &str) -> Result<()>`
  - `pub async fn agent_wait(herdr_bin: &str, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()>`
  - `pub async fn agent_send(herdr_bin: &str, pane_id: &str, text: &str) -> Result<()>`

This task has no failing-test step — it's the untested IO boundary explicitly called out in the design doc (`docs/superpowers/specs/2026-08-05-implement-on-enter-design.md`'s Testing strategy section), matching the existing `open::that(url)` call's status. Its correctness gate is a clean `cargo build` plus the manual verification in Task 6.

- [ ] **Step 1: Write the module**

Create `src/plugin/herdr_cli.rs`:

```rust
//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `start_implementation`) to open a tab, start an
//! agent, wait for it to become ready, and inject text. Deliberately untested at this layer —
//! same status as the existing `open::that(url)` call for the `o` key; see
//! docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for why.

use crate::error::Error;
use crate::Result;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

/// Result of a successful `herdr agent start` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStarted {
    pub pane_id: String,
    pub tab_id: String,
}

/// Resolve the `herdr` binary path: `$HERDR_BIN_PATH`, falling back to `"herdr"` on `$PATH` —
/// the same convention `scripts/open-tab.sh` uses.
pub fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

/// Run a `herdr` CLI subcommand, returning the parsed `result` field on success. Maps a
/// non-zero exit, an `{"error": ...}` response, or unparseable JSON to `Error::Internal` with
/// the CLI's own error message (or raw stderr/stdout as a fallback) so failures are always
/// actionable in the status banner they end up in.
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    let output = Command::new(herdr_bin)
        .args(args)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to run `{herdr_bin}`: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();

    if !output.status.success() {
        let message = parsed
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            });
        return Err(Error::Internal(format!(
            "`{herdr_bin} {}` failed: {message}",
            args.join(" ")
        )));
    }

    let parsed = parsed.ok_or_else(|| {
        Error::Internal(format!(
            "`{herdr_bin} {}` returned unparseable output: {stdout}",
            args.join(" ")
        ))
    })?;

    parsed.get("result").cloned().ok_or_else(|| {
        Error::Internal(format!(
            "`{herdr_bin} {}` had no `result` field",
            args.join(" ")
        ))
    })
}

/// `herdr agent list` — the raw JSON text of the `result` field, for
/// [`crate::plugin::implement::resolve_preferred_agent`] to parse.
pub async fn agent_list(herdr_bin: &str) -> Result<String> {
    let result = run(herdr_bin, &["agent", "list"]).await?;
    Ok(result.to_string())
}

/// `herdr agent start <name> --cwd <cwd> --focus -- <argv...>` — starts `name` (used by herdr
/// for its own agent-status tracking) running `argv` in a fresh, focused tab at `cwd`.
pub async fn agent_start(
    herdr_bin: &str,
    name: &str,
    cwd: &Path,
    argv: &[String],
) -> Result<AgentStarted> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["agent", "start", name, "--cwd", &cwd_str, "--focus", "--"];
    for a in argv {
        args.push(a.as_str());
    }
    let result = run(herdr_bin, &args).await?;

    let pane_id = result
        .get("agent")
        .and_then(|a| a.get("pane_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Error::Internal("agent.start response missing agent.pane_id".to_string())
        })?
        .to_string();
    let tab_id = result
        .get("agent")
        .and_then(|a| a.get("tab_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Internal("agent.start response missing agent.tab_id".to_string()))?
        .to_string();

    Ok(AgentStarted { pane_id, tab_id })
}

/// `herdr tab rename <tab_id> <label>`.
pub async fn tab_rename(herdr_bin: &str, tab_id: &str, label: &str) -> Result<()> {
    run(herdr_bin, &["tab", "rename", tab_id, label])
        .await
        .map(|_| ())
}

/// `herdr agent wait <pane_id> --status <status> --timeout <timeout_ms>`.
pub async fn agent_wait(herdr_bin: &str, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()> {
    let timeout_str = timeout_ms.to_string();
    run(
        herdr_bin,
        &[
            "agent",
            "wait",
            pane_id,
            "--status",
            status,
            "--timeout",
            &timeout_str,
        ],
    )
    .await
    .map(|_| ())
}

/// `herdr agent send <pane_id> <text>`.
pub async fn agent_send(herdr_bin: &str, pane_id: &str, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "send", pane_id, text])
        .await
        .map(|_| ())
}
```

- [ ] **Step 2: Register the module**

In `src/plugin/mod.rs`, add `pub mod herdr_cli;` (alphabetically, after `data` and before `implement`):

```rust
pub mod app;
pub mod config;
pub mod data;
pub mod herdr_cli;
pub mod implement;
pub mod launch;
pub mod repo;
pub mod ui;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build --features plugin`
Expected: builds cleanly, no warnings from this file. (`cargo clippy --all-targets --all-features -- -D warnings` — matches `just lint` — should also pass; run it too.)

- [ ] **Step 4: Commit**

```bash
git add src/plugin/herdr_cli.rs src/plugin/mod.rs
git commit -m "feat: add herdr CLI subprocess wrapper (TF-584)"
```

---

### Task 4: `Action::Implement`, `<Enter>` handling, and the status banner

**Files:**
- Modify: `src/models.rs`
- Modify: `src/plugin/app.rs`

**Interfaces:**
- Consumes: `crate::Issue` (now `PartialEq`).
- Produces (consumed by Task 5's `ui.rs` and Task 6's `main.rs`):
  - `Action::Implement(Issue)` variant
  - `App::status(&self) -> Option<(&str, bool)>`
  - `App::set_status(&mut self, text: String, is_error: bool)`
  - `App::clear_status(&mut self)`
  - `handle_key` returns `Some(Action::Implement(issue.clone()))` for `KeyCode::Enter` on a non-empty loaded view.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/app.rs`'s `#[cfg(test)] mod tests` block, after the existing `o_key_on_an_empty_list_returns_no_action` test:

```rust
    #[test]
    fn enter_key_returns_implement_action_with_the_selected_issue() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Enter);

        assert_eq!(
            action,
            Some(Action::Implement(sample_issue("ENG-1")))
        );
    }

    #[test]
    fn enter_key_in_a_view_on_an_empty_list_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        assert_eq!(handle_key(&mut app, KeyCode::Enter), None);
    }

    #[test]
    fn app_starts_with_no_status() {
        let app = App::new();
        assert_eq!(app.status(), None);
    }

    #[test]
    fn set_status_stores_the_message_and_error_flag() {
        let mut app = App::new();

        app.set_status("started".to_string(), false);
        assert_eq!(app.status(), Some(("started", false)));

        app.set_status("boom".to_string(), true);
        assert_eq!(app.status(), Some(("boom", true)));
    }

    #[test]
    fn clear_status_removes_it() {
        let mut app = App::new();
        app.set_status("started".to_string(), false);

        app.clear_status();

        assert_eq!(app.status(), None);
    }

    #[test]
    fn returning_to_the_menu_clears_status() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status("started".to_string(), false);

        handle_key(&mut app, KeyCode::Esc);

        assert_eq!(app.status(), None);
    }

    #[test]
    fn entering_a_view_from_the_menu_clears_status() {
        let mut app = App::new();
        app.set_status("stale".to_string(), true);

        app.enter_selected_menu_option();

        assert_eq!(app.status(), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --all-features --lib plugin::app -- --nocapture`
Expected: compile error — `Action::Implement`, `App::status`/`set_status`/`clear_status` don't exist, and `Issue`/`Action` don't implement the comparisons the tests need yet.

- [ ] **Step 3: Implement**

First, in `src/models.rs`, add `PartialEq` to the derive line of every struct reachable from `Issue` (this is required for `#[derive(PartialEq)]` on `Action` to compile once it holds an `Issue`): `User`, `Team`, `Issue`, `IssueState`, `Project`, `ProjectStatus`, `Cycle`, `Label`, `LabelConnection`. Each currently reads `#[derive(Debug, Clone, Serialize, Deserialize)]` — change each to `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`. (`Comment` and `PageInfo`/`GraphQLResponse`/`Connection<T>` are not reachable from `Issue` — leave them alone.)

Then, in `src/plugin/app.rs`:

Add the `status` field to `App` and initialize it in `new()`:

```rust
pub struct App {
    /// The current screen.
    screen: Screen,
    /// A transient status banner shown under an issue list — separate from
    /// `ViewState::Error` so it doesn't discard an already-loaded issue list. Set by the
    /// `Action::Implement` orchestration in `main.rs` as it progresses through the
    /// implement-on-Enter flow. `(message, is_error)`.
    status: Option<(String, bool)>,
}
```

```rust
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
            status: None,
        }
    }
```

Add the three status methods anywhere in `impl App` (e.g. right after `selected_issue`):

```rust
    /// The current status banner, if any: `(message, is_error)`.
    pub fn status(&self) -> Option<(&str, bool)> {
        self.status
            .as_ref()
            .map(|(text, is_error)| (text.as_str(), *is_error))
    }

    /// Sets the status banner, replacing any existing one.
    pub fn set_status(&mut self, text: String, is_error: bool) {
        self.status = Some((text, is_error));
    }

    /// Clears the status banner.
    pub fn clear_status(&mut self) {
        self.status = None;
    }
```

Clear it on the two transitions the design calls out — add `self.status = None;` to `return_to_menu`:

```rust
    pub fn return_to_menu(&mut self) {
        self.screen = Screen::Menu { selected: 0 };
        self.status = None;
    }
```

and to `enter_selected_menu_option`, right before it returns `Some(Action::EnterView)`:

```rust
        self.screen = Screen::View(option.kind, ViewState::Loading);
        self.status = None;
        Some(Action::EnterView)
```

Add the `Implement` variant to `Action`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    OpenInBrowser(String),
    Retry,
    /// A menu option was entered; the caller should trigger a data fetch for the
    /// now-current view (see [`App::current_view`]).
    EnterView,
    /// `<Enter>` was pressed on a selected issue: open a herdr tab, start the preferred
    /// coding agent, set the issue to "In Progress", and inject the implement prompt once
    /// ready. Orchestrated in `main.rs`'s `start_implementation`.
    Implement(Issue),
}
```

Add the `KeyCode::Enter` arm to the view-level match in `handle_key` (the second `match key { ... }` block, alongside `Char('o')`):

```rust
        KeyCode::Enter => app
            .selected_issue()
            .map(|issue| Action::Implement(issue.clone())),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib -- --nocapture`
Expected: PASS — the new `plugin::app` tests, plus every pre-existing test across the crate (this is the step that proves the `models.rs` `PartialEq` addition didn't break anything elsewhere, e.g. `client.rs`'s tests deserializing these same types).

- [ ] **Step 5: Commit**

```bash
git add src/models.rs src/plugin/app.rs
git commit -m "feat: add Action::Implement, <Enter> handling, and status banner (TF-584)"
```

---

### Task 5: Render the status banner

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: `App::status(&self) -> Option<(&str, bool)>` (Task 4).

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/ui.rs`'s `#[cfg(test)] mod tests` block, after `renders_issue_identifier_and_title_in_the_list`:

```rust
    #[test]
    fn renders_the_status_banner_when_present() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status("ENG-1: tab opened, agent started, set to In Progress.".to_string(), false);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1: tab opened, agent started, set to In Progress."));
    }

    #[test]
    fn renders_an_error_status_banner() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status("ENG-1: failed to start agent tab: boom".to_string(), true);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1: failed to start agent tab: boom"));
    }

    #[test]
    fn renders_without_a_status_banner_when_none_is_set() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        // No status set — the list/detail view alone must still render exactly as before.
        let text = rendered_text(&app);
        assert!(text.contains("ENG-1"));
        assert!(text.contains("Title for ENG-1"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features --lib plugin::ui -- --nocapture`
Expected: FAIL — `renders_the_status_banner_when_present` and `renders_an_error_status_banner` fail their `assert!` (the banner text is never rendered yet); `renders_without_a_status_banner_when_none_is_set` already passes (it's a regression guard for the change about to be made).

- [ ] **Step 3: Implement**

In `src/plugin/ui.rs`, change `draw` to pass the status through, and `draw_view` to accept and render it:

```rust
pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => draw_view(frame, *kind, view_state, app.status()),
    }
}
```

```rust
fn draw_view(
    frame: &mut Frame,
    kind: ViewKind,
    view_state: &ViewState,
    status: Option<(&str, bool)>,
) {
    match view_state {
        ViewState::Loading => {
            let paragraph = Paragraph::new("Loading issues...")
                .block(Block::default().borders(Borders::ALL).title("Linear"));
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Error { message } => {
            let paragraph = Paragraph::new(format!("{message}\n\nPress r to retry."))
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Linear - Error"),
                );
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Loaded { issues, selected } => {
            let area = if let Some((text, is_error)) = status {
                let outer = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(1)])
                    .split(frame.area());
                let style = if is_error {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                frame.render_widget(Paragraph::new(text).style(style), outer[1]);
                outer[0]
            } else {
                frame.area()
            };

            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);

            let items: Vec<ListItem> = issues
                .iter()
                .map(|issue| ListItem::new(format!("{} {}", issue.identifier, issue.title)))
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(kind.label()))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            let mut list_state = ListState::default();
            list_state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[0], &mut list_state);

            let detail = issues
                .get(*selected)
                .map(|issue| {
                    format!(
                        "{}\n\n{}\n\nState: {}\nURL: {}",
                        issue.identifier, issue.title, issue.state.name, issue.url
                    )
                })
                .unwrap_or_default();
            let detail_widget = Paragraph::new(detail)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title("Detail"));
            frame.render_widget(detail_widget, chunks[1]);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::ui -- --nocapture`
Expected: PASS — all tests, including the three new ones and every pre-existing `ui.rs` test (the `Some`/`None` status branching keeps the no-status layout byte-for-byte equivalent to before).

- [ ] **Step 5: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: render the status banner under the issue list (TF-584)"
```

---

### Task 6: Wire the orchestration into `main.rs`

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `plugin::config::load_agent_command_override` (Task 1), `plugin::implement::{resolve_preferred_agent, resolve_agent_command, build_shell_argv, build_implement_prompt, pick_in_progress_state}` (Task 2), `plugin::herdr_cli::{herdr_bin, agent_list, agent_start, tab_rename, agent_wait, agent_send, AgentStarted}` (Task 3), `plugin::app::{Action::Implement, App::set_status}` (Task 4), `herdr_linear::LinearClient::{get_workflow_states, update_issue}` (existing).
- Produces: `async fn start_implementation(app: &mut plugin::app::App, client: &herdr_linear::LinearClient, issue: herdr_linear::Issue)`, called from `event_loop`'s new `Action::Implement` arm. Nothing downstream depends on it — this is the top of the call graph for this feature.

No automated tests for this task (it's pure orchestration over the untested `herdr_cli` boundary — see Task 3 and the design doc's Testing strategy). Verify with the manual recipe in Step 3.

- [ ] **Step 1: Add the `serde_json::json` import**

At the top of `src/main.rs`, add:

```rust
use serde_json::json;
```

- [ ] **Step 2: Implement `start_implementation` and wire it in**

Add this function to `src/main.rs`, after `ensure_loaded` and before `event_loop`:

```rust
/// Runs the full "implement this issue" flow triggered by `<Enter>` on a selected issue:
/// resolve the preferred coding agent, open a herdr tab running it, set the issue to its
/// team's "In Progress" state, wait for the agent to become ready, then inject the implement
/// prompt. Every failure sets a specific, actionable status banner on `app` instead of
/// propagating — mirrors `ensure_loaded`'s "inline error instead of crashing" philosophy. See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full data flow.
async fn start_implementation(
    app: &mut plugin::app::App,
    client: &herdr_linear::LinearClient,
    issue: herdr_linear::Issue,
) {
    let herdr_bin = plugin::herdr_cli::herdr_bin();

    let agent_list_json = match plugin::herdr_cli::agent_list(&herdr_bin).await {
        Ok(json) => json,
        Err(err) => {
            app.set_status(format!("{}: {err}", issue.identifier), true);
            return;
        }
    };
    let derived = plugin::implement::resolve_preferred_agent(&agent_list_json);

    let config_override = match plugin::config::load_agent_command_override() {
        Ok(value) => value,
        Err(err) => {
            app.set_status(format!("{}: {err}", issue.identifier), true);
            return;
        }
    };

    let command =
        plugin::implement::resolve_agent_command(derived.as_deref(), config_override.as_deref());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let argv = plugin::implement::build_shell_argv(&shell, &command);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let started = match plugin::herdr_cli::agent_start(&herdr_bin, &command, &cwd, &argv).await {
        Ok(started) => started,
        Err(err) => {
            app.set_status(
                format!("{}: failed to start agent tab: {err}", issue.identifier),
                true,
            );
            return;
        }
    };

    let mut warnings = Vec::new();

    if let Err(err) =
        plugin::herdr_cli::tab_rename(&herdr_bin, &started.tab_id, &issue.identifier).await
    {
        warnings.push(format!("failed to rename tab: {err}"));
    }

    match client.get_workflow_states(&issue.team.id).await {
        Ok(states) => match plugin::implement::pick_in_progress_state(&states) {
            Some(state) => {
                let updates = json!({ "stateId": state.id });
                if let Err(err) = client.update_issue(&issue.id, updates).await {
                    warnings.push(format!("failed to set state to In Progress: {err}"));
                }
            }
            None => {
                warnings.push("no \"In Progress\"-equivalent workflow state found".to_string())
            }
        },
        Err(err) => warnings.push(format!("failed to load workflow states: {err}")),
    }

    let prompt = plugin::implement::build_implement_prompt(&issue.identifier);

    if let Err(err) =
        plugin::herdr_cli::agent_wait(&herdr_bin, &started.pane_id, "idle", 30_000).await
    {
        app.set_status(
            format!(
                "{}: agent didn't become ready ({err}) — run manually: {prompt}",
                issue.identifier
            ),
            true,
        );
        return;
    }

    if let Err(err) = plugin::herdr_cli::agent_send(&herdr_bin, &started.pane_id, &prompt).await {
        app.set_status(
            format!(
                "{}: failed to send implement command ({err}) — run manually: {prompt}",
                issue.identifier
            ),
            true,
        );
        return;
    }

    if warnings.is_empty() {
        app.set_status(
            format!(
                "{}: tab opened, agent started, set to In Progress.",
                issue.identifier
            ),
            false,
        );
    } else {
        app.set_status(
            format!("{}: started, but {}", issue.identifier, warnings.join("; ")),
            true,
        );
    }
}
```

Then add the `Action::Implement` arm to `event_loop`'s match, right after the existing `Retry | EnterView` arm:

```rust
                        plugin::app::Action::Retry | plugin::app::Action::EnterView => {
                            // `handle_key` already moved `app` into `Loading` — either
                            // retrying the current view or entering a newly selected
                            // one; draw that before the fetch's own round-trip so
                            // it's visible instead of leaving the stale previous frame.
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            ensure_loaded(app, client).await;
                        }
                        plugin::app::Action::Implement(issue) => {
                            app.set_status(
                                format!("Starting implementation for {}…", issue.identifier),
                                false,
                            );
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            match client.as_ref() {
                                Some(c) => start_implementation(app, c, issue).await,
                                None => app.set_status(
                                    format!(
                                        "{}: not connected to Linear yet — try again.",
                                        issue.identifier
                                    ),
                                    true,
                                ),
                            }
                        }
```

- [ ] **Step 3: Verify — automated, then manual**

Run: `cargo test --all-features -- --nocapture`
Expected: PASS — the whole suite, including everything from Tasks 1–5.

Run: `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all -- --check` (equivalently: `just check`, but note `just check` runs `fmt` in write mode, not `--check` — run `just check` directly, it's the project's real pre-commit gate).
Expected: clean.

Manual verification against real herdr panes (there is no automated coverage for `herdr_cli`/`start_implementation` — this is the actual test):

```bash
just plugin-reinstall
```

Then, with the plugin panel open in a herdr workspace that has a valid `api_key` configured:

1. **Happy path:** select an issue, press `<Enter>`. Confirm: a new focused tab opens titled with the issue identifier; the agent starts in it; within ~30s the injected `Implement Linear Issue <identifier> using a new git worktree` text appears in that pane; back in the Linear panel, the status banner reads "...tab opened, agent started, set to In Progress."; refreshing the issue in Linear shows it moved to "In Progress".
2. **Agent-wait timeout:** temporarily set `HERDR_BIN_PATH` to a wrapper script that starts a plain shell instead of a real agent (so `agent_status` never reports `idle` the way a recognized agent would) and confirm the status banner shows the "agent didn't become ready" message with the literal fallback prompt text, and nothing was silently lost.
3. **Bad `api_key`:** temporarily break `config.toml`'s `api_key`, press `<Enter>` on an issue, confirm the tab still opens and the agent still starts, but the status banner reports "failed to load workflow states: ..." (or the state-update failure) while everything else succeeded — the partial-failure, no-rollback behavior from the design.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire the implement-on-Enter orchestration into the event loop (TF-584)"
```

---

## Self-Review

**Spec coverage:**
- New `<Enter>` action, `o` untouched → Task 4 (handle_key), Task 6 (event_loop keeps `Action::OpenInBrowser` arm as-is).
- Real `issueUpdate` mutation to the team's "In Progress" state → Task 6, using the existing `get_workflow_states`/`update_issue` plus Task 2's `pick_in_progress_state`.
- Tab title = issue identifier → Task 6's `tab_rename` call.
- Preferred-agent derivation from other tabs + configurable fallback (default `hr`) → Task 1 (config) + Task 2 (`resolve_preferred_agent`/`resolve_agent_command`).
- Tab opening via the herdr CLI (socket), analogous to existing launcher patterns → Task 3 (`herdr_cli`), using the same `$HERDR_BIN_PATH` convention as `scripts/open-tab.sh`.
- Command injection only after confirming the agent is ready → Task 6's `agent_wait` call gating `agent_send`.
- Clear, non-silent error messages for state-update / tab-open / agent-start failures → Task 6's per-step `set_status(..., true)` calls; Task 5 renders them.
- Unit tests for the pure/deterministic parts; socket interaction isolated/untested like `launch.rs` → Tasks 1, 2, 4 (tests); Task 3 deliberately untested, matching `launch.rs`'s own zero-socket-interaction precedent (see design doc's Testing strategy).

**Placeholder scan:** no TBD/TODO; every step has real, complete code; no "similar to Task N" references — Task 3/5/6 each repeat exactly what they need rather than pointing elsewhere.

**Type consistency:** `AgentStarted { pane_id, tab_id }` (Task 3) is used identically in Task 6 (`started.tab_id`, `started.pane_id`). `App::status()` returns `Option<(&str, bool)>` (Task 4) and is consumed with that exact shape in Task 5's `draw_view` and Task 6's assertions-by-proxy (manual verification). `pick_in_progress_state`'s `Option<&IssueState>` (Task 2) is consumed via `.id` in Task 6, matching `IssueState`'s field name from `models.rs`.
