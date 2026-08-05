# Implement-on-Enter — design

**Date:** 2026-08-05
**Status:** Approved
**Linear issue:** [TF-584](https://linear.app/talent-factory/issue/TF-584)

## Problem

Today, selecting an issue in the issue list (`My Issues` / `Project Issues` / `Team Issues`) and
pressing `<Enter>` does nothing — only `o` (open in browser) is wired up. We want `<Enter>` to kick
off the full "start implementing this issue" flow: open a new herdr tab titled after the issue,
flip the issue to `In Progress` via a real GraphQL mutation, start the user's preferred coding
agent in that tab, and — once the agent is actually ready — inject
`Implement Linear Issue <identifier> using a new git worktree`.

## Scope

- New `<Enter>` key handling in the issue list, alongside the existing `o`/`r`/`Esc`/`q` handling.
  `o` (open in browser) is untouched.
- Real `issueUpdate` GraphQL mutation to move the issue to its team's "In Progress"-equivalent
  workflow state (`update_issue` already exists in `client.rs`; only the state lookup + call site
  are new).
- Preferred-agent resolution: primarily derived from other open herdr tabs (via `herdr agent
  list`); falls back to a configurable `agent_command` in `config.toml` (default `hr`) when no
  other agent tabs exist.
- Tab creation + agent start + readiness wait + text injection, all via the `herdr` CLI's JSON
  socket protocol (never a raw socket — same boundary the existing launcher scripts use).
- Clear, non-destructive error surfacing for every failure mode named in the acceptance criteria.

Out of scope: worktree creation itself (delegated to the agent via the injected prompt), any new
Linear state beyond the single `issueUpdate` call, and rollback/undo of partially-succeeded steps.

## Architecture

Two new modules, following the existing pure-logic/thin-IO split already used by
`plugin::launch` (pure) vs. the launcher shell scripts (IO):

- **`plugin::implement`** — pure functions only, fully unit-tested, no process/socket access.
- **`plugin::herdr_cli`** — thin wrapper shelling out to the `herdr` binary
  (`$HERDR_BIN_PATH` or `"herdr"`, matching `scripts/open-tab.sh`'s convention) and parsing its
  JSON stdout. Deliberately untested at this layer, same status as the existing `open::that(url)`
  call for `o` — there is no lower-level abstraction worth mocking here without inventing new test
  infrastructure the rest of the codebase doesn't have.

`plugin::config` is extended with one more optional field, following the exact pattern
`project_id` already established.

### `src/plugin/app.rs` (extended)

- `Action` gains `Implement(Issue)` (mirrors `OpenInBrowser(String)`, but the flow needs
  `id`, `identifier`, and `team.id`, so it carries the full (already-`Clone`) `Issue`).
- `handle_key` maps `KeyCode::Enter` in a loaded view to
  `app.selected_issue().map(|issue| Action::Implement(issue.clone()))` — `None` on an empty list,
  matching the existing `o` handling.
- `App` gains a transient status banner: `status: Option<(String, bool /* is_error */)>`, with
  `set_status(text, is_error)` / `clear_status()`. This is deliberately separate from
  `ViewState::Error` — a hard error there blows away the loaded issue list and forces a reload via
  `r`, which is wrong for a banner over an otherwise-healthy list. `Esc` (return to menu) and
  entering a new view clear it; starting a new `Implement` flow immediately overwrites it.

### `src/plugin/config.rs` (extended)

- `ConfigFile` gains `agent_command: Option<String>`.
- `resolve_agent_command_override(config_dir: Option<&Path>) -> Result<Option<String>>` — mirrors
  `resolve_project_id_override` exactly (empty/whitespace-only treated as "no override").
- `load_agent_command_override() -> Result<Option<String>>` — thin wrapper over
  `$HERDR_PLUGIN_CONFIG_DIR`, mirrors `load_project_id_override`.

### `src/plugin/implement.rs` (new)

Pure functions, unit-tested:

- `resolve_preferred_agent(agent_list_json: &str) -> Option<String>` — parses the `herdr agent
  list` JSON result, collects each pane's `agent` field (skipping `null`/absent — our own TUI pane
  never reports one, so no self-exclusion is needed), and returns the most frequent value, ties
  broken by first-seen order. Malformed JSON or an empty list → `None` (falls through to config).
- `resolve_agent_command(derived: Option<&str>, config_override: Option<&str>) -> String` —
  precedence: `derived` → `config_override` → `"hr"`.
- `build_shell_argv(shell: &str, command: &str) -> Vec<String>` → `[shell, "-i", "-c", command]`.
  Always shell-wrapped, uniformly for both a bare derived binary (`claude`) and a config alias
  (`hr`, confirmed to be a zsh alias — `alias hr='headroom wrap claude --memory --code-graph'` —
  not a PATH executable), so there's exactly one exec path instead of two.
- `build_implement_prompt(identifier: &str) -> String` →
  `format!("Implement Linear Issue {identifier} using a new git worktree")`.
- `pick_in_progress_state<'a>(states: &'a [IssueState]) -> Option<&'a IssueState>` — case-
  insensitive name match on `"In Progress"` first, else the first `type == "started"` state, else
  `None`.

### `src/plugin/herdr_cli.rs` (new)

Thin subprocess wrapper, one function per socket method used, each running
`Command::new(herdr_bin).args([...]).output()` and mapping non-zero exit / unparseable JSON /
top-level `"error"` responses to `Error::Internal(...)`:

- `agent_list(herdr_bin: &str) -> Result<String>` — returns the raw JSON `result` blob (parsing is
  `implement::resolve_preferred_agent`'s job, not this module's).
- `agent_start(herdr_bin: &str, name: &str, cwd: &Path, argv: &[String], focus: bool) ->
  Result<AgentStarted>` where `AgentStarted { pane_id: String, tab_id: String }`, extracted from
  the `agent_started` result's nested `agent.pane_id` / `agent.tab_id`.
- `tab_rename(herdr_bin: &str, tab_id: &str, label: &str) -> Result<()>`.
- `agent_wait(herdr_bin: &str, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()>`.
- `agent_send(herdr_bin: &str, pane_id: &str, text: &str) -> Result<()>`.

## Data flow

New `async fn start_implementation(app: &mut App, client: &LinearClient, issue: Issue)` in
`main.rs`, invoked from `event_loop`'s new `Action::Implement(issue)` arm:

1. `app.set_status(format!("Starting implementation for {}…", issue.identifier), false)`, redraw.
2. `herdr_cli::agent_list(herdr_bin)` → on error, set an error status and return (nothing has
   side-effected yet, so this is a cheap abort).
3. `implement::resolve_preferred_agent(&json)` → `derived`.
4. `config::load_agent_command_override()` → on `Err` (malformed TOML), set an error status and
   return, same as the existing config-error handling in `ensure_loaded`.
5. `command = implement::resolve_agent_command(derived.as_deref(), override.as_deref())`.
6. `shell = $SHELL or "/bin/bash"`; `argv = implement::build_shell_argv(&shell, &command)`.
7. `cwd = std::env::current_dir()` (same cwd `repo::detect_repo_name` already uses).
8. `herdr_cli::agent_start(herdr_bin, &command, &cwd, &argv, true)` → on error, set an error
   status ("Failed to start agent tab: …") and return.
9. `herdr_cli::tab_rename(herdr_bin, &started.tab_id, &issue.identifier)` → on error, **do not
   abort**; remember a warning fragment to append to the final status (the agent is already
   running, which matters more than its tab label).
10. `client.get_workflow_states(&issue.team.id)` → `implement::pick_in_progress_state` → if found,
    `client.update_issue(&issue.id, json!({"stateId": state.id}))`. Any error in this step (fetch
    or mutation) is recorded as a warning fragment, **not** a hard abort — steps 11–12 still run.
11. `herdr_cli::agent_wait(herdr_bin, &started.pane_id, "idle", 30_000)` → on error/timeout, set an
    error status that includes the literal prompt text as a manual fallback (nothing is lost) and
    return.
12. `herdr_cli::agent_send(herdr_bin, &started.pane_id, &implement::build_implement_prompt(&issue.identifier))`
    → on error, same manual-fallback error status.
13. On full success (no warning fragments), set a short success status; with warning fragments,
    set an error-flagged status summarizing what succeeded and what didn't.

## Error handling

Every failure mode named in the acceptance criteria maps to a specific `app.set_status(..., true)`
call — never a silent no-op, never a panic. No rollback: a partially-succeeded run (e.g. tab open
+ agent started, but the Linear state mutation failed) is left as-is with a clear message telling
the user what still needs manual attention, matching how the rest of the plugin already prefers
"degrade to a clear inline message" over "hide the problem" (see `launch.rs`'s malformed-JSON →
`OPEN` fallback, and `config.rs`'s distinct error messages per failure cause).

`herdr_cli` failures reuse `Error::Internal` — no new `Error` variant, consistent with how
`config.rs` and `client.rs` already reuse the existing variants rather than growing the enum per
call site.

## Testing strategy

- `plugin::implement`: table-driven unit tests for agent-derivation (tie-breaking, empty/malformed
  JSON → `None`), command-resolution precedence (all three levels), shell-argv construction,
  prompt-string construction, and state-picking (name match / type fallback / no match at all).
  No socket, no process spawn — pure input → output, same spirit as `plugin::launch`'s tests.
- `plugin::config`: `agent_command` override tests mirror the existing `project_id` override tests
  (present, absent, empty/whitespace, malformed TOML) almost verbatim.
- `plugin::app`: `Action::Implement` construction from `KeyCode::Enter` (empty list → `None`,
  non-empty → `Some(Action::Implement(selected_issue))`), and status banner set/clear transitions.
- `plugin::herdr_cli` and `start_implementation` in `main.rs`: not unit tested, same as the rest of
  `main.rs`'s orchestration and the existing `OpenInBrowser` → `open::that` call. Verified manually
  via `herdr plugin link .` + `just plugin-reinstall`, exercising all three response paths (full
  success, agent-wait timeout, Linear mutation failure) against real herdr panes.

## Out of scope / open items for the implementation plan

- Exact wording/formatting of the status banner (single line vs. wrapped) — a `ui.rs` rendering
  detail, not a design-level decision.
- Whether `agent_wait`'s 30s timeout should be configurable — start with a hardcoded constant;
  revisit if it proves too short/long in practice.
- No handling for a team with *zero* workflow states of any kind (`get_workflow_states` returning
  empty) beyond `pick_in_progress_state` returning `None`, which is folded into the same "state
  update failed" warning path as any other lookup miss.
