# Guaranteed Tab-Per-Issue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every `<Enter>`-triggered Linear issue implementation land in its own, correctly and
permanently labeled top-level herdr tab, instead of sometimes landing as a split inside — and
relabeling — whatever tab currently has focus (including a different, still-running issue's tab).

**Architecture:** Replace the current `agent_start` (implicit placement) → `tab_rename`
(after-the-fact relabel) sequence in `start_implementation` with `tab_create` (explicit,
pre-labeled, focused tab) → `agent_start` (now required to target that tab via `--tab`). No new
modules; two existing files change: `src/plugin/herdr_cli.rs` (the `herdr` CLI wrapper) and
`src/main.rs` (`start_implementation`'s orchestration).

**Tech Stack:** Rust, tokio (async subprocess spawning), serde_json (parsing `herdr` CLI JSON
responses), the `herdr` CLI's JSON socket protocol (invoked as a subprocess, never a raw socket).

## Global Constraints

- No rollback on partial failure: every step still reports a specific, actionable status banner
  and leaves prior state as-is — never silently retries, undoes, or hides an earlier warning.
- Tab label stays the bare `issue.identifier` (e.g. `"TF-579"`) — no repo prefix.
- `herdr_bin` resolution stays `$HERDR_BIN_PATH`, falling back to `"herdr"` on `$PATH` (existing
  `herdr_bin()` helper, unchanged).
- The subprocess-spawning half of `herdr_cli` stays deliberately untested at the unit level (see
  the module's own doc comment) — only the pure `parse_*`/`interpret_output` functions get unit
  tests. This matches the rest of the module: `run`, `agent_start`, `agent_wait`, etc. are not
  unit tested; only their pure parsing/decision helpers are.
- Every `cargo test` / `cargo build` run in this plan is `cd`'d to the repo root
  (`/Users/daniel/GitRepository/herdr-linear`), not a worktree copy.

---

### Task 1: Add `herdr_cli::tab_create`

**Files:**
- Modify: `src/plugin/herdr_cli.rs:177-223` (add a new pure parser + a new public async function,
  next to the existing `parse_agent_started`/`agent_start` pair; do not touch `agent_start` or
  `tab_rename` in this task — that's Task 2)
- Test: same file, `#[cfg(test)] mod tests` block at the bottom (inline unit tests, matching every
  other test in this module)

**Interfaces:**
- Consumes: `TabId` (existing newtype, `struct TabId(String)`, already defined earlier in this
  file with `as_str()`/`Display`), `Error::Internal` (existing variant), `run()` (existing private
  helper — `async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value>`), `Result<T>` (crate's
  `type Result<T> = std::result::Result<T, Error>` alias).
- Produces: `pub async fn tab_create(herdr_bin: &str, cwd: &Path, label: &str) -> Result<TabId>` —
  Task 2's `start_implementation` call site depends on this exact signature and on `TabId` being
  the returned type it then threads into `agent_start`.

- [ ] **Step 1: Write the failing tests for `parse_tab_created`**

Add this test module content inside the existing `#[cfg(test)] mod tests { use super::*; ... }`
block in `src/plugin/herdr_cli.rs`, right after the existing `parse_agent_started_*` tests (so it
sits next to the function it mirrors):

```rust
    #[test]
    fn parse_tab_created_extracts_the_tab_id() {
        let result = serde_json::json!({
            "tab": {"tab_id": "wY:t2D", "label": "TF-579"},
            "root_pane": {"pane_id": "wY:p31"}
        });

        let tab_id = parse_tab_created(&result).unwrap();

        assert_eq!(tab_id.as_str(), "wY:t2D");
    }

    #[test]
    fn parse_tab_created_errors_when_tab_id_is_missing() {
        let result = serde_json::json!({"tab": {"label": "TF-579"}});

        let err = parse_tab_created(&result).unwrap_err().to_string();

        assert!(err.contains("tab.tab_id"), "unexpected message: {err}");
    }

    #[test]
    fn parse_tab_created_errors_when_the_tab_object_is_missing_entirely() {
        let result = serde_json::json!({"root_pane": {"pane_id": "wY:p31"}});

        assert!(parse_tab_created(&result).is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `cargo test --lib parse_tab_created`
Expected: FAIL — `error[E0425]: cannot find function `parse_tab_created` in this scope` (the
function doesn't exist yet).

- [ ] **Step 3: Implement `parse_tab_created` and `tab_create`**

Add this above `agent_start` in `src/plugin/herdr_cli.rs` (right after the existing
`parse_agent_started` function, before its `agent_start` doc comment/definition):

```rust
/// Extract the created [`TabId`] from a `herdr tab create` call's already-unwrapped `result`
/// value. Split out from [`tab_create`] for the same testability reason as
/// [`parse_agent_started`].
fn parse_tab_created(result: &Value) -> Result<TabId> {
    result
        .get("tab")
        .and_then(|t| t.get("tab_id"))
        .and_then(|v| v.as_str())
        .map(|s| TabId(s.to_string()))
        .ok_or_else(|| Error::Internal("tab.create response missing tab.tab_id".to_string()))
}

/// `herdr tab create --cwd <cwd> --label <label> --focus` — creates a fresh, focused tab that is
/// already labeled `label`, and returns its [`TabId`]. Labeling at creation time (rather than via
/// a follow-up `tab rename`) means the label is correct from the very first frame, with no window
/// in which the tab could be confused with — or have its label stolen by — a different,
/// already-running tab. See
/// docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for why this replaced a
/// `tab_rename`-after-`agent_start` sequence.
pub async fn tab_create(herdr_bin: &str, cwd: &Path, label: &str) -> Result<TabId> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let result = run(
        herdr_bin,
        &[
            "tab", "create", "--cwd", &cwd_str, "--label", label, "--focus",
        ],
    )
    .await?;
    parse_tab_created(&result)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib parse_tab_created`
Expected: PASS — 3 tests (`parse_tab_created_extracts_the_tab_id`,
`parse_tab_created_errors_when_tab_id_is_missing`,
`parse_tab_created_errors_when_the_tab_object_is_missing_entirely`), all `ok`.

- [ ] **Step 5: Run the full test suite to confirm nothing else broke**

Run: `cargo test --lib`
Expected: PASS — every existing test in the crate still passes; the 3 new ones are included in the
total count.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/herdr_cli.rs
git commit -m "feat: add herdr_cli::tab_create for explicit, pre-labeled tab placement

Purely additive — nothing calls it yet. Task 2 wires it into
start_implementation in place of the agent_start -> tab_rename
sequence.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 2: Wire `tab_create` into `start_implementation`, require an explicit tab on `agent_start`, delete `tab_rename`

**Files:**
- Modify: `src/plugin/herdr_cli.rs:135-139` (module-doc list of functions routed through `run`'s
  default timeout — update to reflect the function set after this task)
- Modify: `src/plugin/herdr_cli.rs:201-223` (`agent_start`'s signature + doc comment, and deletion
  of `tab_rename`)
- Modify: `src/main.rs:152-172` (doc comment on `start_implementation` — drop the now-inaccurate
  "tab rename" mention from the warnings list)
- Modify: `src/main.rs:325-343` (the `agent_start` → `tab_rename` block inside
  `start_implementation`, replaced with `tab_create` → `agent_start`)

**Interfaces:**
- Consumes: `plugin::herdr_cli::tab_create` (from Task 1, signature
  `async fn tab_create(herdr_bin: &str, cwd: &Path, label: &str) -> Result<TabId>`).
- Produces: `agent_start`'s new signature —
  `pub async fn agent_start(herdr_bin: &str, name: &str, cwd: &Path, tab: &TabId, argv: &[String]) -> Result<AgentStarted>`
  (was: `(herdr_bin: &str, name: &str, cwd: &Path, argv: &[String])`, no `tab` param). Any other
  code that calls `agent_start` must be updated in this same task — the crate must compile at the
  end of it. `tab_rename` no longer exists after this task; nothing outside this task may reference
  it (confirmed via `grep -rn "tab_rename" src/` returning zero matches after Step 3 below).

- [ ] **Step 1: Update the module-doc function list**

In `src/plugin/herdr_cli.rs`, find:

```rust
/// Wall-clock ceiling for `herdr` subprocess calls that don't carry their own `--timeout`
/// argument (everything routed through [`run`]: `agent_list`, `agent_start`, `tab_rename`,
/// `agent_send`). Without this, a hung `herdr` daemon blocks the single-threaded TUI's event
/// loop indefinitely — `agent_wait` is the exception, since it computes its own call-specific
/// bound in [`agent_wait`] instead of using this constant.
```

Replace the second line so the function list matches what actually exists after this task:

```rust
/// argument (everything routed through [`run`]: `agent_list`, `tab_create`, `agent_start`,
```

- [ ] **Step 2: Change `agent_start`'s signature to require a `TabId`, and delete `tab_rename`**

Replace the current `agent_start` function and the `tab_rename` function immediately after it
(`src/plugin/herdr_cli.rs:201-223`) with:

```rust
/// `herdr agent start <name> --cwd <cwd> --tab <tab> --focus -- <argv...>` — starts `name` (used
/// by herdr for its own agent-status tracking) running `argv` at `cwd`, explicitly placed inside
/// `tab` (created via [`tab_create`]) rather than trusting herdr's own default placement — see
/// docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for why an implicit
/// placement previously let one issue's agent land as a split inside a different, already-running
/// issue's tab. There is deliberately no variant of this function that omits `tab`.
pub async fn agent_start(
    herdr_bin: &str,
    name: &str,
    cwd: &Path,
    tab: &TabId,
    argv: &[String],
) -> Result<AgentStarted> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec![
        "agent", "start", name, "--cwd", &cwd_str, "--tab", tab.as_str(), "--focus", "--",
    ];
    for a in argv {
        args.push(a.as_str());
    }
    let result = run(herdr_bin, &args).await?;
    parse_agent_started(&result)
}
```

(Note: this deletes the old `/// \`herdr tab rename <tab_id> <label>\`.` doc comment and its
`tab_rename` function body entirely — nothing replaces it.)

- [ ] **Step 3: Confirm `tab_rename` has no remaining references**

Run: `grep -rn "tab_rename" src/`
Expected: no output (zero matches). If anything other than `src/main.rs`'s call site turns up,
stop and investigate before continuing — Step 2 assumed `tab_rename` had exactly one caller.

- [ ] **Step 4: Update `start_implementation`'s doc comment**

In `src/main.rs`, find (around line 152-161):

```rust
/// Runs the full "implement this issue" flow triggered by `<Enter>` on a selected issue:
/// resolve the preferred coding agent, open a herdr tab running it, set the issue to its
/// team's "In Progress" state, wait for the agent to become ready, then inject the implement
/// prompt. Every failure sets a specific, actionable status banner on `app` instead of
/// propagating — mirrors `ensure_loaded`'s "inline error instead of crashing" philosophy. Any
/// non-fatal warnings collected along the way (tab rename, workflow-state lookup, the actual
/// state transition) are preserved in *every* terminal status, not just the final success case
/// — a failure late in the flow (e.g. `agent_wait` timing out) must not hide an earlier one
/// (e.g. the issue never actually reaching "In Progress"). See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full data flow.
```

Replace the "non-fatal warnings" line and add a pointer to the new design doc:

```rust
/// Runs the full "implement this issue" flow triggered by `<Enter>` on a selected issue:
/// resolve the preferred coding agent, create a fresh tab labeled after the issue and start it
/// running there, set the issue to its team's "In Progress" state, wait for the agent to become
/// ready, then inject the implement prompt. Every failure sets a specific, actionable status
/// banner on `app` instead of propagating — mirrors `ensure_loaded`'s "inline error instead of
/// crashing" philosophy. Any non-fatal warnings collected along the way (workflow-state lookup,
/// the actual state transition) are preserved in *every* terminal status, not just the final
/// success case — a failure late in the flow (e.g. `agent_wait` timing out) must not hide an
/// earlier one (e.g. the issue never actually reaching "In Progress"). See
/// docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for the full original data flow
/// and docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for the
/// tab-creation change.
```

- [ ] **Step 5: Replace the `agent_start` → `tab_rename` block with `tab_create` → `agent_start`**

In `src/main.rs`, find (around line 325-343):

```rust
    let started =
        match plugin::herdr_cli::agent_start(&herdr_bin, command.as_str(), &cwd, &argv).await {
            Ok(started) => started,
            Err(err) => {
                app.set_status(plugin::app::Status::Error(format!(
                    "{}: failed to start agent tab: {err}",
                    issue.identifier
                )));
                return;
            }
        };

    let mut warnings = Vec::new();

    if let Err(err) =
        plugin::herdr_cli::tab_rename(&herdr_bin, &started.tab_id, &issue.identifier).await
    {
        warnings.push(format!("failed to rename tab: {err}"));
    }
```

Replace it with:

```rust
    let tab_id = match plugin::herdr_cli::tab_create(&herdr_bin, &cwd, &issue.identifier).await {
        Ok(tab_id) => tab_id,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: failed to create a tab: {err}",
                issue.identifier
            )));
            return;
        }
    };

    let started = match plugin::herdr_cli::agent_start(
        &herdr_bin,
        command.as_str(),
        &cwd,
        &tab_id,
        &argv,
    )
    .await
    {
        Ok(started) => started,
        Err(err) => {
            app.set_status(plugin::app::Status::Error(format!(
                "{}: tab created but agent failed to start ({err}) — an empty '{}' tab was left \
                 open, close it manually",
                issue.identifier, issue.identifier
            )));
            return;
        }
    };

    let mut warnings = Vec::new();
```

Everything after this block (`client.get_workflow_states(...)` onward, through the end of
`start_implementation`) is unchanged — leave it exactly as it is.

- [ ] **Step 6: Build and run the full test suite**

Run: `cargo build --all-features && cargo test --all-features -- --nocapture`
Expected: PASS — clean build, every existing test (including `status_with_warnings_*` in
`src/main.rs`, unaffected by this change since it only tests generic message-joining) plus Task
1's 3 new tests, all `ok`.

- [ ] **Step 7: Run the project's full quality gate**

Run: `just check`
Expected: `✅ All checks passed!` (runs `cargo fmt --all`, `cargo clippy --all-targets
--all-features -- -D warnings`, then the test suite again).

- [ ] **Step 8: Commit**

```bash
git add src/plugin/herdr_cli.rs src/main.rs
git commit -m "fix: create a labeled tab before starting the agent, instead of renaming after

agent_start() never told herdr where to place the new agent pane, so
it inherited herdr's implicit default placement — often a split into
whatever tab currently had focus, which could be a different,
already-running issue's tab. The follow-up tab_rename() call would
then blindly relabel that tab, silently stealing its title.

start_implementation now calls the new tab_create() first (labels the
tab atomically at creation time, no rename race), then passes that
tab's id into agent_start(), which now requires an explicit --tab and
can no longer fall back to herdr's default. tab_rename() is deleted —
it had exactly one caller and nothing replaces it.

See docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: Manual verification against a live herdr instance

**Files:** none (no code changes — this task only runs and observes the built plugin)

**Interfaces:**
- Consumes: the release binary built from Task 2's changes, via `just plugin-reinstall`.

This task's checks can't be automated — they depend on a running `herdr` server, a real terminal
session, and (for the "In Progress" transition) live Linear issues — matching the project's
existing convention that `start_implementation` and the rest of `herdr_cli`'s subprocess-spawning
half stay manually, not unit, verified.

- [ ] **Step 1: Rebuild and relink the plugin**

Run: `just plugin-reinstall`
Expected: ends with `✅ Plugin reinstalled`.

- [ ] **Step 2: Open the Linear panel and implement two different issues back to back**

In a herdr session inside this repo, open the Linear panel (however you normally trigger it —
`open-split.sh`/`open-tab.sh`), select one issue and press `<Enter>`, wait for its agent tab to
reach the "idle" status banner (`"...: tab opened, agent started, set to In Progress."` — wording
now comes from the same success branch as before, unchanged by this plan), then go back to the
Linear panel, select a **different** issue, and press `<Enter>` again.

- [ ] **Step 3: Confirm both issues have their own, correctly labeled tab**

Run: `herdr tab list` (or inspect the tab bar directly)
Expected: two separate tabs exist, one labeled with each issue's identifier (e.g. `TF-580` and
`TF-579`), each showing exactly one agent pane (`pane_count: 1` if checked via
`herdr tab get <id>`) — neither tab shows a split with the other issue's session, and neither
issue's original tab label was overwritten by the other.

- [ ] **Step 4: Confirm the failure-path message by pointing `HERDR_BIN_PATH` at a broken binary**

Run (in the same shell the plugin process will inherit): `HERDR_BIN_PATH=/bin/false` set before
launching herdr, or temporarily rename the real `herdr` binary — then trigger `<Enter>` on an
issue.
Expected: status banner reads `"<identifier>: failed to create a tab: ..."` (the `tab_create`
failure path — a cheap abort, no tab or agent was created). Restore `HERDR_BIN_PATH`/the renamed
binary afterward.

- [ ] **Step 5: No commit for this task** — it's verification only; if any check fails, return to
  Task 2 and fix before proceeding.
