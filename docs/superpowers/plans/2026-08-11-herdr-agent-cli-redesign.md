# herdr Agent-CLI Redesign (TF-624) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make herdr-linear's `c` keybinding (open config.toml) and Implement-on-Enter work again against herdr >= 0.8.0, whose `agent start`/`agent wait`/`agent send` CLI surface changed shape out from under this plugin — and document the verified-compatible herdr version in `CHANGELOG.md`/`README.md`/`herdr-plugin.toml` once it does.

**Architecture:** Replace the "one call spawns a named agent with arbitrary argv in a fresh tab" model (`herdr agent start <name> --cwd --tab --focus -- <argv>`, now rejected with `unknown option: --cwd`) with the primitives the new herdr actually exposes: `tab create` (unchanged) to get a pane at a shell prompt, a new `pane run <pane> <command>` wrapper to type the launch command into it (works for arbitrary commands — wrapper aliases, `nvim` — none of which fit the new `agent start --kind <fixed-enum>` model), then herdr's own passive auto-detection plus the existing (flag-fixed) `agent_wait`/renamed `agent_prompt` to know when a *recognized* coding agent is ready. The config-editor reuse-on-second-press check moves from `agent focus <name>` (impossible for `nvim`, which can never become a recognized "agent") to a `tab list` + label match + `tab focus`, mirroring the pattern `scripts/open-tab.sh` already uses for panel reuse. This also deletes now-unreachable complexity: TF-590's `agent_name_taken` retry loop and the TF-579 redundant-root-pane-close dance, neither of which apply once the process runs directly in `tab_create`'s root pane instead of a `agent_start`-created split.

**Tech Stack:** Rust, tokio, serde_json, the `herdr` CLI's JSON socket protocol (subprocess-invoked).

## Global Constraints

- Every fact this plan relies on about herdr's current CLI shape was verified live against the installed herdr 0.8.0 binary during investigation (not read from herdr's own source, which isn't available locally) — see TF-624's description for the verified command transcripts.
- `min_herdr_version` in `herdr-plugin.toml` (mirrored by `MIN_HERDR_VERSION` in `herdr_cli.rs`) is raised from `"0.7.0"` to `"0.8.0"` by Task 8 — the new `pane_run`/`tab_list`/`tab_focus`/`agent_rename`/`agent_prompt`/`agent_wait --until` calls this plan introduces have only ever been verified against 0.8.0, so publishing the old floor would be inaccurate. `tab_create`'s `--cwd`/`--label`/`--focus` flags themselves are unchanged by this plan and still work exactly as before.
- No placeholder/TODO code. Every step below is the literal diff to make.
- This crate gates all of `src/plugin/*` (including `herdr_cli.rs`) and the binary itself behind the `plugin` Cargo feature (`default = []` in `Cargo.toml`) — every build/test/lint command in this plan must include `--all-features` (or use the project's own `justfile` recipes, which already do: `just fmt`, `just lint` = `cargo clippy --all-targets --all-features -- -D warnings`, `just test` = `cargo test --all-features -- --nocapture`, `just check` = all three). A bare `cargo test`/`cargo clippy --all-targets -- -D warnings` without `--all-features` silently skips this code entirely — discovered live during Task 1's review (TF-624), when it let a genuine `dead_code` clippy failure through undetected. Individual task steps below that say plain `cargo test`/`cargo clippy`/`cargo build` mean the `--all-features`/`just`-recipe form; run `just check` after every task and do not move to the next task with a red build.
- Reference ticket: TF-624.

---

### Task 1: Add `pane_run`, `tab_list`/`find_tab_id_by_label`, `tab_focus`, `agent_rename` to `herdr_cli.rs`

**Files:**
- Modify: `src/plugin/herdr_cli.rs`
- Test: `src/plugin/herdr_cli.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub async fn pane_run(herdr_bin: &str, pane_id: &PaneId, command: &str) -> Result<()>`, `pub async fn tab_list(herdr_bin: &str) -> Result<String>`, `fn find_tab_id_by_label(tab_list_json: &str, label: &str) -> Option<TabId>` (private, pure), `pub async fn tab_focus(herdr_bin: &str, tab_id: &TabId) -> Result<()>`, `pub async fn agent_rename(herdr_bin: &str, pane_id: &PaneId, name: &str) -> Result<()>`.
- Consumes: existing `run`, `PaneId`, `TabId` from this same file.

- [ ] **Step 1: Add the four new wrapper functions**, right after the existing `pane_close` function (around line 553-558):

```rust
/// `herdr pane run <pane_id> <command>` — types `command` into `pane_id` followed by Enter.
/// The pane must already be at an interactive shell prompt (the state every `tab_create` root
/// pane starts in). Unlike the old `agent_start`, this works for *any* command — an arbitrary
/// `agent_command` wrapper alias (e.g. `"hr"`), or `nvim`, neither of which fit herdr 0.8.0's
/// `agent start --kind <fixed-enum>` model. herdr detects and tracks whatever recognized coding
/// agent binary ends up running purely by passive observation of the pane — no separate
/// registration call is needed or possible for an unrecognized command like `nvim`.
pub async fn pane_run(herdr_bin: &str, pane_id: &PaneId, command: &str) -> Result<()> {
    run(herdr_bin, &["pane", "run", pane_id.as_str(), command])
        .await
        .map(|_| ())
}

/// `herdr tab list` — the raw JSON text of the `result` field. Used by
/// [`find_tab_id_by_label`] to locate an existing labeled tab (the config-editor pane's
/// reuse-on-second-`c`-press check) — `agent focus`/`agent rename` can't do this for `nvim`,
/// since herdr only tracks *recognized* coding-agent binaries as "agents", never a plain
/// editor pane.
pub async fn tab_list(herdr_bin: &str) -> Result<String> {
    let result = run(herdr_bin, &["tab", "list"]).await?;
    Ok(result.to_string())
}

/// Find the `tab_id` of the first tab in a `herdr tab list` JSON result (the already-unwrapped
/// `result` value, serialized back to text by [`tab_list`]) whose `label` exactly matches
/// `label`. Returns `None` on unparseable JSON, an empty/missing `tabs` array, or no match —
/// all of which mean "nothing to reuse, create a fresh one" to every caller. Pure — no I/O — so
/// the matching logic is unit-testable without spawning a process, mirroring
/// [`crate::plugin::implement::resolve_preferred_agent`]'s same split for `agent list`.
///
/// `#[allow(dead_code)]`: this task (Task 1, TF-624) lands the function and its unit tests
/// ahead of its only real caller, [`find_existing_editor_tab`], which Task 5 adds — without the
/// allow, `cargo clippy --all-features` fails this task's own commit in isolation (verified live
/// during Task 1's review). Task 5 removes this attribute in the same edit that adds the caller.
#[allow(dead_code)]
fn find_tab_id_by_label(tab_list_json: &str, label: &str) -> Option<TabId> {
    let parsed: Value = serde_json::from_str(tab_list_json).ok()?;
    let tabs = parsed.get("tabs")?.as_array()?;
    tabs.iter().find_map(|tab| {
        let tab_label = tab.get("label")?.as_str()?;
        if tab_label != label {
            return None;
        }
        let tab_id = tab.get("tab_id")?.as_str()?;
        Some(TabId(tab_id.to_string()))
    })
}

/// `herdr tab focus <tab_id>`. Used to switch to an already-open config-editor tab on a second
/// `c` press, in place of the old `agent focus <name>` (impossible for `nvim` — see
/// [`find_tab_id_by_label`]'s doc).
pub async fn tab_focus(herdr_bin: &str, tab_id: &TabId) -> Result<()> {
    run(herdr_bin, &["tab", "focus", tab_id.as_str()])
        .await
        .map(|_| ())
}

/// `herdr agent rename <pane_id> <name>` — assigns a friendly display name to a pane herdr has
/// already recognized as hosting a coding agent (requires the target to already be
/// auto-detected; fails with `agent_not_found` otherwise — verified live against herdr 0.8.0).
/// Used by [`crate::implement_one`] purely cosmetically, to preserve the per-issue names
/// (`hr--tf-574`-style) users already see in herdr's own pane/agent list — TF-590's original
/// motivation (avoiding a launch-time `agent_name_taken` collision) no longer applies, since
/// nothing about [`pane_run`] can collide on a name.
pub async fn agent_rename(herdr_bin: &str, pane_id: &PaneId, name: &str) -> Result<()> {
    run(herdr_bin, &["agent", "rename", pane_id.as_str(), name])
        .await
        .map(|_| ())
}
```

- [ ] **Step 2: Add unit tests for `find_tab_id_by_label`** in the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn find_tab_id_by_label_returns_the_matching_tab_id() {
    let json = r#"{"tabs":[{"tab_id":"w1:t1","label":"Terminal"},{"tab_id":"w1:t2","label":"herdr-linear-config"}]}"#;

    let found = find_tab_id_by_label(json, "herdr-linear-config");

    assert_eq!(found, Some(TabId("w1:t2".to_string())));
}

#[test]
fn find_tab_id_by_label_returns_none_when_no_tab_matches() {
    let json = r#"{"tabs":[{"tab_id":"w1:t1","label":"Terminal"}]}"#;

    assert_eq!(find_tab_id_by_label(json, "herdr-linear-config"), None);
}

#[test]
fn find_tab_id_by_label_returns_none_on_unparseable_json() {
    assert_eq!(find_tab_id_by_label("not json", "herdr-linear-config"), None);
}

#[test]
fn find_tab_id_by_label_returns_none_when_tabs_array_is_missing() {
    assert_eq!(find_tab_id_by_label(r#"{}"#, "herdr-linear-config"), None);
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --all-features find_tab_id_by_label -- --nocapture` (must include `--all-features` — this module only compiles under the `plugin` feature, see Global Constraints), then `just check` to confirm the whole task's diff is clean (fmt + `clippy --all-targets --all-features -- -D warnings` + full test suite).
Expected: 4 passed.

- [ ] **Step 4: Commit**

```bash
git add src/plugin/herdr_cli.rs
git commit -m "feat: add pane_run/tab_list/tab_focus/agent_rename herdr_cli wrappers (TF-624)"
```

---

### Task 2: Fix `agent_wait`'s `--status` → `--until`, rename `agent_send` → `agent_prompt`

**Files:**
- Modify: `src/plugin/herdr_cli.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub async fn agent_prompt(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()>` (replaces `agent_send`, same signature).

- [ ] **Step 1: Fix the flag in `agent_wait`** — in the `agent_wait` function body, change:

```rust
                "agent",
                "wait",
                pane_id.as_str(),
                "--status",
                status,
```

to:

```rust
                "agent",
                "wait",
                pane_id.as_str(),
                "--until",
                status,
```

Update the function's doc comment line `/// \`herdr agent wait <pane_id> --status <status> --timeout <timeout_ms>\`.` to `/// \`herdr agent wait <pane_id> --until <status> --timeout <timeout_ms>\` (herdr 0.8.0 renamed \`--status\` to \`--until\`, TF-624).`

- [ ] **Step 2: Rename `agent_send` to `agent_prompt`**, replacing:

```rust
/// `herdr agent send <pane_id> <text>`.
pub async fn agent_send(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "send", pane_id.as_str(), text])
        .await
        .map(|_| ())
}
```

with:

```rust
/// `herdr agent prompt <pane_id> <text>` (herdr 0.8.0 replaced the old `agent send` subcommand
/// with `agent prompt`, which additionally supports `--wait`/`--until`/`--timeout` options this
/// plugin doesn't need — it does its own stability polling via `agent_read` instead, see
/// `main.rs`'s `send_prompt_until_visible`).
pub async fn agent_prompt(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "prompt", pane_id.as_str(), text])
        .await
        .map(|_| ())
}
```

- [ ] **Step 3: Update `agent_read`'s doc comment** — it currently says `to confirm an [\`agent_send\`] actually reached...`; change the link to `[\`agent_prompt\`]`.

- [ ] **Step 4: Update every call site** — `grep -rn "herdr_cli::agent_send" src/` and change each to `herdr_cli::agent_prompt` (main.rs's `send_prompt_until_visible`, covered fully in Task 6 Step 3).

- [ ] **Step 5: Run tests to verify nothing else references the old names**

Run: `cargo build 2>&1 | grep -i "agent_send\|--status"`
Expected: no output (Task 5 will fix the main.rs call site; if this build step runs before Task 5 is done, a `agent_send` reference here is expected and resolved there).

- [ ] **Step 6: Commit**

```bash
git add src/plugin/herdr_cli.rs
git commit -m "fix: agent_wait --status to --until, rename agent_send to agent_prompt (TF-624)"
```

---

### Task 3: Remove `agent_start` and the now-dead `agent_name_taken` retry machinery

**Files:**
- Modify: `src/plugin/herdr_cli.rs`
- Modify: `src/error.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing new — this is pure deletion. `AgentStarted`, `agent_start`, `run_for_agent_start`, `parse_agent_started`, `next_name_taken_retry`, `AGENT_START_NAME_TAKEN_MAX_RETRIES`, `parse_agent_name_taken_error`, `Error::AgentNameTaken` all go away. Callers are fixed in Tasks 4 and 5.

- [ ] **Step 1: Delete these items from `src/plugin/herdr_cli.rs`** (in file order):
  - The `AgentStarted` struct (lines ~64-69).
  - `next_name_taken_retry` and its doc comment, and `AGENT_START_NAME_TAKEN_MAX_RETRIES` (lines ~415-435).
  - `agent_start` in its entirety (the whole function, roughly lines 465-560).
  - `parse_agent_started` (lines ~347-368).
  - `parse_agent_name_taken_error` (lines ~198-243).
  - `run_for_agent_start` (lines ~343-349).

- [ ] **Step 2: Simplify `interpret_output`/`run_with_timeout`/`run`** — remove the now-unused `check_agent_name_taken: bool` parameter and its call-site branch:

```rust
fn interpret_output(command_desc: &str, status_success: bool, stdout: &str, stderr: &str) -> Result<Value> {
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let error_obj = parsed.as_ref().and_then(|v| v.get("error"));

    let error_message = error_obj
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string);

    if !status_success || error_message.is_some() {
        let message = error_message.unwrap_or_else(|| {
            let stderr = stderr.trim();
            if stderr.is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.to_string()
            }
        });
        let message = match unsupported_cwd_flag_hint(&message) {
            Some(hint) => format!("{message} — {hint}"),
            None => message,
        };
        return Err(Error::Internal(format!(
            "`{command_desc}` failed: {message}"
        )));
    }

    let parsed = parsed.ok_or_else(|| {
        Error::Internal(format!(
            "`{command_desc}` returned unparseable output: {}",
            stdout.trim()
        ))
    })?;

    parsed
        .get("result")
        .cloned()
        .ok_or_else(|| Error::MissingResultField(format!("`{command_desc}` had no `result` field")))
}
```

Update its doc comment to drop the `check_agent_name_taken`/`agent_start` paragraph entirely (keep the rest: non-zero exit / error body / unparseable-JSON mapping, and the `unsupported_cwd_flag_hint` note — that hint still applies to `tab_create`, the one remaining `--cwd`-accepting call).

`run_with_timeout` drops the same parameter and its pass-through:

```rust
async fn run_with_timeout(herdr_bin: &str, args: &[&str], call_timeout: Duration) -> Result<Value> {
    let command_desc = format!("{herdr_bin} {}", args.join(" "));

    let output = tokio::time::timeout(call_timeout, spawn_with_etxtbsy_retry(herdr_bin, args))
        .await
        .map_err(|_| {
            Error::Internal(format!(
                "`{command_desc}` timed out after {call_timeout:?} waiting for herdr"
            ))
        })?
        .map_err(|e| Error::Internal(format!("Failed to run `{herdr_bin}`: {e}")))?;

    interpret_output(
        &command_desc,
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}
```

`run` drops the trailing `false`:

```rust
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(herdr_bin, args, DEFAULT_CLI_TIMEOUT).await
}
```

Update `DEFAULT_CLI_TIMEOUT`'s doc comment (it currently lists `agent_start`/`agent_send` among callers via `run`) to `agent_list, tab_create, agent_prompt, pane_close, pane_run, tab_list, tab_focus, agent_rename`.

- [ ] **Step 3: Fix every remaining `interpret_output`/`run_with_timeout` call in the test module** to drop the trailing bool argument — `grep -n "interpret_output(\|run_with_timeout(" src/plugin/herdr_cli.rs` and update each call.

- [ ] **Step 4: Remove `Error::AgentNameTaken`** from `src/error.rs` — delete the variant (lines ~54-76) and its two dedicated tests (`agent_name_taken_display_omits_candidates_suffix_when_empty`, `agent_name_taken_display_appends_candidates_when_present`).

- [ ] **Step 5: Update the module-level doc comment** at the top of `herdr_cli.rs` — replace:

```
//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `implement_one`, shared by both the single- and
//! multi-issue callers) to open a tab, start an agent, wait for it to become ready, and inject
//! text.
```

with:

```
//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `implement_one`, shared by both the single- and
//! multi-issue callers) to open a tab, type the launch command into it, wait for the resulting
//! agent to become ready, and inject text — and by the `c` keybinding's
//! `open_config_in_herdr_pane` to open/reuse a config-editor tab the same way.
```

- [ ] **Step 6: Run tests**

Run: `cargo build --all-targets 2>&1 | grep -E "^error"`
Expected: errors only in `main.rs` (call sites fixed in Tasks 4-5) — no errors remaining in `herdr_cli.rs`/`error.rs` themselves.

- [ ] **Step 7: Commit**

```bash
git add src/plugin/herdr_cli.rs src/error.rs
git commit -m "refactor: remove agent_start and the now-unreachable agent_name_taken retry logic (TF-624)"
```

---

### Task 4: Editor argv → shell-quoted command string

**Files:**
- Modify: `src/plugin/editor.rs`

**Interfaces:**
- Produces: `pub fn build_editor_command(editor_cmd: &str, config_path: &Path) -> String` (replaces `build_editor_argv`).
- Consumes: nothing new.

- [ ] **Step 1: Replace `build_editor_argv` with `build_editor_command`**, which single-quote-shell-escapes the path (needed because [`pane_run`](herdr_cli.rs) types this straight into a live shell — a config dir containing a space, while unlikely for this plugin's fixed `$HERDR_PLUGIN_CONFIG_DIR`, must not split into two shell words):

```rust
/// Shell-quotes `s` for safe interpolation into a command typed via `pane_run`: wraps it in
/// single quotes, escaping any embedded single quote as `'\''` (the standard POSIX-shell
/// technique — close the quote, emit an escaped literal quote, reopen the quote).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Builds the shell command line to type into a fresh pane (via
/// `crate::plugin::herdr_cli::pane_run`) to launch `editor_cmd` on `config_path`. Unlike the old
/// `build_editor_argv` (removed, TF-624) this returns one shell-quoted string, not an argv
/// vector — `pane_run` types text into an already-interactive shell rather than exec'ing argv
/// directly.
pub fn build_editor_command(editor_cmd: &str, config_path: &Path) -> String {
    format!("{editor_cmd} {}", shell_quote(&config_path.to_string_lossy()))
}
```

- [ ] **Step 2: Replace the existing `build_editor_argv_pairs_the_command_with_the_config_path` test** with:

```rust
#[test]
fn build_editor_command_joins_the_editor_and_shell_quoted_config_path() {
    let command = build_editor_command("nvim", Path::new("/fake/config/dir/config.toml"));

    assert_eq!(command, "nvim '/fake/config/dir/config.toml'");
}

#[test]
fn build_editor_command_escapes_an_embedded_single_quote_in_the_path() {
    let command = build_editor_command("nvim", Path::new("/fake/o'brien/config.toml"));

    assert_eq!(command, r"nvim '/fake/o'\''brien/config.toml'");
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib build_editor_command`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add src/plugin/editor.rs
git commit -m "refactor: build_editor_argv to build_editor_command (shell-quoted, for pane_run) (TF-624)"
```

---

### Task 5: Rewrite `open_config_in_herdr_pane` (the `c` keybinding) in `main.rs`

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `herdr_cli::{tab_list, tab_focus, tab_create, pane_run, TabCreated, TabId, PaneId}` (Task 1), `plugin::editor::{build_editor_command, EDITOR_AGENT_NAME}` (Task 4).
- Produces: `open_config_in_herdr_pane` keeps its existing signature and `Result<(), HerdrPaneError>` return type — `open_config_editor` (its only caller) is unaffected.

- [ ] **Step 1: Replace the whole `open_config_in_herdr_pane` function body**:

```rust
async fn open_config_in_herdr_pane(
    herdr_bin: &str,
    editor_cmd: &str,
    config_path: &std::path::Path,
) -> std::result::Result<(), HerdrPaneError> {
    // Reuse an already-open editor tab from a previous `c` press, if any. `nvim` can never
    // become a herdr-recognized "agent" (it's not in herdr's fixed `--kind` enum, and herdr only
    // auto-detects/tracks recognized coding-agent binaries — verified live against herdr 0.8.0,
    // TF-624), so `agent focus` can never find it; a tab-label lookup is the only reuse
    // mechanism that still works, mirroring `scripts/open-tab.sh`'s existing panel-reuse
    // pattern.
    match plugin::herdr_cli::tab_list(herdr_bin).await {
        Ok(json) => {
            if let Some(tab_id) = plugin::herdr_cli::find_existing_editor_tab(&json) {
                return plugin::herdr_cli::tab_focus(herdr_bin, &tab_id)
                    .await
                    .map_err(|err| {
                        HerdrPaneError::Unavailable(format!(
                            "found an existing '{}' tab but failed to focus it: {err}",
                            plugin::editor::EDITOR_AGENT_NAME
                        ))
                    });
            }
        }
        Err(err) => {
            tracing::debug!(
                "couldn't list tabs to check for an existing '{}' pane ({err}) — creating a new \
                 tab",
                plugin::editor::EDITOR_AGENT_NAME
            );
        }
    }

    let cwd = config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(plugin::host::resolve_cwd);
    let command = plugin::editor::build_editor_command(editor_cmd, config_path);

    let created_tab = plugin::herdr_cli::tab_create(herdr_bin, &cwd, plugin::editor::EDITOR_AGENT_NAME)
        .await
        .map_err(|err| HerdrPaneError::Unavailable(format!("failed to create a tab: {err}")))?;

    // A `pane_run` `Err` doesn't necessarily mean the editor never launched — the same
    // `run_with_timeout`-without-`kill_on_drop` caveat `agent_start`'s old doc described applies
    // here too (a client-side timeout on a `herdr` call that's still running in the background).
    // Report `Ambiguous`, not `Unavailable`, so the caller doesn't fall back to the OS opener and
    // risk opening the file a second time.
    plugin::herdr_cli::pane_run(herdr_bin, &created_tab.root_pane_id, &command)
        .await
        .map_err(|err| {
            HerdrPaneError::Ambiguous(format!(
                "tab created but launching the editor failed ({err}) — check the '{}' tab: it \
                 may be empty (safe to close) or the editor may have started anyway despite the \
                 error, so verify before closing it",
                plugin::editor::EDITOR_AGENT_NAME
            ))
        })
}
```

Note this drops the old `agent_focus` reuse-check, the `AgentStarted`/`started.pane_id != created_tab.root_pane_id` redundant-pane-close dance entirely — the editor now always runs directly in `tab_create`'s own root pane via `pane_run` (no split is ever created, since nothing calls `agent_start` anymore), so there is no redundant pane to close.

- [ ] **Step 2: Add `find_existing_editor_tab` to `herdr_cli.rs`** (thin wrapper around Task 1's `find_tab_id_by_label`, made `pub` since `main.rs` needs it and `find_tab_id_by_label` itself stays private/pure for unit testing). This is `find_tab_id_by_label`'s first real caller — **remove the `#[allow(dead_code)]` attribute Task 1 put directly above `fn find_tab_id_by_label`** in the same edit (its explanatory comment says exactly this; delete both the comment and the attribute line, leaving the function's original doc comment intact):

```rust
/// Convenience wrapper around [`find_tab_id_by_label`] for [`crate::open_config_in_herdr_pane`]:
/// looks for an existing tab labeled [`crate::plugin::editor::EDITOR_AGENT_NAME`] in a `tab
/// list` JSON result.
pub fn find_existing_editor_tab(tab_list_json: &str) -> Option<TabId> {
    find_tab_id_by_label(tab_list_json, crate::plugin::editor::EDITOR_AGENT_NAME)
}
```

- [ ] **Step 3: Update the `HerdrPaneError` enum's doc comment** — it references `agent_start`'s handling; change the `see \`agent_start\`'s handling below` reference to `see \`pane_run\`'s handling above`.

- [ ] **Step 4: Replace `write_editor_herdr_script` and its five dependent tests** (lines ~1279-1514). New fixture, dispatching on `tab list`/`tab create`/`pane run` instead of `agent focus`/`tab create`/`agent start`/`pane close`:

```rust
/// Fake `herdr` script dispatching `tab list`, `tab create`, and `pane run` calls to canned
/// bodies — the three subcommands `open_config_in_herdr_pane` can issue after TF-624's redesign.
fn write_editor_herdr_script(
    tab_list: &str,
    tab_create: &str,
    pane_run: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    write_fake_herdr_script(&format!(
        r#"
case "$1 $2" in
  "tab list") {tab_list} ;;
  "tab create") {tab_create} ;;
  "pane run") {pane_run} ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
    ))
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_focuses_an_existing_tab_without_creating_a_new_one() {
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"result":{"tabs":[{"tab_id":"w1:t2","label":"herdr-linear-config"}]}}'; exit 0"#,
        r#"echo 'tab create should not run'; exit 1"#,
        r#"echo 'pane run should not run'; exit 1"#,
    );

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    assert_eq!(result, Ok(()));
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_creates_a_tab_when_no_existing_one_is_found() {
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"result":{"tabs":[]}}'; exit 0"#,
        r#"echo '{"result":{"tab":{"tab_id":"t2","label":"herdr-linear-config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
        r#"echo '{"result":{}}'; exit 0"#,
    );

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    assert_eq!(result, Ok(()));
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_threads_the_editor_cmd_and_cwd_into_the_cli_calls() {
    // `write_editor_herdr_script`'s tests above only prove the right subcommand runs in the
    // right order — none of them inspect the argv beyond `$1 $2`. A regression that hardcoded
    // the wrong editor, dropped/swapped `cwd`/`config_path`, or built the wrong shell-quoted
    // command would pass every one of them. This captures every call's full argv instead.
    let capture_dir = tempfile::tempdir().unwrap();
    let args_file = capture_dir.path().join("args.txt");
    let (_dir, script) = write_fake_herdr_script(&format!(
        r#"
printf 'CALL: %s\n' "$*" >> "{args_file}"
case "$1 $2" in
  "tab list")
    echo '{{"result":{{"tabs":[]}}}}'
    exit 0
    ;;
  "tab create")
    echo '{{"result":{{"tab":{{"tab_id":"t2","label":"herdr-linear-config"}},"root_pane":{{"pane_id":"p9"}}}}}}'
    exit 0
    ;;
  "pane run")
    echo '{{"result":{{}}}}'
    exit 0
    ;;
esac
"#,
        args_file = args_file.display()
    ));

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    assert_eq!(result, Ok(()));

    let captured = std::fs::read_to_string(&args_file).unwrap();
    assert_eq!(
        captured,
        "CALL: tab list\n\
         CALL: tab create --cwd /fake/config/dir --label herdr-linear-config --focus\n\
         CALL: pane run p9 nvim '/fake/config/dir/config.toml'\n",
        "unexpected sequence of herdr CLI calls: {captured}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_treats_an_unlistable_tabs_call_as_no_existing_tab() {
    // `tab list` failing outright (rather than succeeding with an empty/non-matching list) must
    // fall through to creating a fresh tab, not bubble up as a hard failure.
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
        r#"echo '{"result":{"tab":{"tab_id":"t2","label":"herdr-linear-config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
        r#"echo '{"result":{}}'; exit 0"#,
    );

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    assert_eq!(result, Ok(()));
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_fails_when_tab_create_fails_after_no_existing_tab_is_found() {
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"result":{"tabs":[]}}'; exit 0"#,
        r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
        r#"echo 'pane run should not run'; exit 1"#,
    );

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    let Err(HerdrPaneError::Unavailable(message)) = result else {
        panic!("expected Err(Unavailable), got {result:?}");
    };
    assert!(
        message.contains("failed to create a tab") && message.contains("no such workspace"),
        "unexpected message: {message}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn open_config_in_herdr_pane_returns_ambiguous_when_pane_run_fails_after_tab_create() {
    // `pane_run`'s `Err` doesn't mean the editor never launched (see this function's own doc on
    // `run_with_timeout`'s lack of `kill_on_drop`) — so this must be reported as `Ambiguous`, not
    // `Unavailable`, or the caller would fall back to the OS opener and risk opening the file a
    // second time.
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"result":{"tabs":[]}}'; exit 0"#,
        r#"echo '{"result":{"tab":{"tab_id":"t2","label":"herdr-linear-config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
        r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
    );

    let result = open_config_in_herdr_pane(
        script.to_str().unwrap(),
        "nvim",
        std::path::Path::new("/fake/config/dir/config.toml"),
    )
    .await;

    let Err(HerdrPaneError::Ambiguous(message)) = result else {
        panic!("expected Err(Ambiguous), got {result:?}");
    };
    assert!(
        message.contains("no such pane")
            && message.contains("check the")
            && message.contains(plugin::editor::EDITOR_AGENT_NAME),
        "unexpected message: {message}"
    );
}
```

- [ ] **Step 5: Fix the two `open_config_editor_*` tests** (lines ~1516-1560+) that also call `write_editor_herdr_script` — update their call sites to the new 3-argument signature:

```rust
#[cfg(unix)]
#[tokio::test]
async fn open_config_editor_does_not_call_the_opener_when_the_herdr_pane_succeeds() {
    let (_dir, script) = write_editor_herdr_script(
        r#"echo '{"result":{"tabs":[{"tab_id":"w1:t2","label":"herdr-linear-config"}]}}'; exit 0"#,
        r#"echo 'tab create should not run'; exit 1"#,
        r#"echo 'pane run should not run'; exit 1"#,
    );
    let opener_calls = std::cell::RefCell::new(Vec::new());

    let result = open_config_editor(
        std::path::Path::new("/fake/config/dir/config.toml"),
        Some("nvim".to_string()),
        script.to_str().unwrap(),
        |p| {
            opener_calls.borrow_mut().push(p.to_path_buf());
            Ok(())
        },
    )
    .await;

    assert_eq!(result, Ok(()));
    assert!(opener_calls.into_inner().is_empty());
}
```

(Leave `open_config_editor_calls_the_opener_when_no_editor_resolved`, which never touches `herdr`, unchanged.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test open_config`
Expected: all `open_config_*` tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/plugin/herdr_cli.rs
git commit -m "fix: redesign open_config_in_herdr_pane (the c keybinding) for herdr 0.8.0 (TF-624)"
```

---

### Task 6: Rewrite `implement_one` (Implement-on-Enter) in `main.rs`

**Files:**
- Modify: `src/main.rs`
- Modify: `src/plugin/implement.rs`

**Interfaces:**
- Consumes: `herdr_cli::{tab_create, pane_run, agent_wait, agent_rename, agent_prompt}` (Tasks 1-2), `plugin::implement::{build_agent_name, is_valid_agent_command, ValidatedAgentCommand, resolve_agent_command, pick_in_progress_state, build_implement_prompt}`.
- Produces: `implement_one` keeps its existing signature and `ImplementOutcome` return type — `implement_many`/`start_implementation`/`start_implementation_many` are unaffected.

- [ ] **Step 1: Remove `build_shell_argv` from `src/plugin/implement.rs`** — no longer needed: `pane_run` types straight into the tab's already-interactive login shell (rc files already sourced), so there's no need to re-wrap the command through a nested `$SHELL -i -c "<command>"`. Delete the function and its doc-comment references to it in `is_valid_agent_command`/`ValidatedAgentCommand`'s docs (reword "so `build_shell_argv` only accepts this type" to "so `pane_run`'s caller only ever receives a validated command"). Delete its test `build_shell_argv_wraps_the_command_through_an_interactive_shell`.

- [ ] **Step 2: Replace the `implement_one` function body** (from `let shell = ...` through the final `pane_close` cleanup, keeping everything from `match client.get_workflow_states(...)` onward unchanged):

```rust
async fn implement_one(
    herdr_bin: &str,
    client: &herdr_linear::LinearClient,
    issue: &herdr_linear::Issue,
    command: &plugin::implement::ValidatedAgentCommand,
) -> ImplementOutcome {
    let cwd = plugin::host::resolve_cwd();
    if cwd.as_os_str().is_empty() {
        return ImplementOutcome::Failed(
            "couldn't determine your working directory (herdr's launch context is missing \
             and the plugin's own process directory is unreadable) — see README.md's \"Use\" \
             section"
                .to_string(),
        );
    }

    let agent_name = plugin::implement::build_agent_name(command.as_str(), &issue.identifier);

    let created_tab = match plugin::herdr_cli::tab_create(herdr_bin, &cwd, &issue.identifier).await
    {
        Ok(created_tab) => created_tab,
        Err(err) => return ImplementOutcome::Failed(format!("failed to create a tab: {err}")),
    };

    // A `pane_run` `Err` does not necessarily mean the agent never started — the most likely
    // cause is `run_with_timeout` giving up on a `herdr` call that's still running in the
    // background (no `kill_on_drop`), so the agent may well be up despite the error. Don't
    // assert the tab is empty; tell the user to check first.
    if let Err(err) =
        plugin::herdr_cli::pane_run(herdr_bin, &created_tab.root_pane_id, command.as_str()).await
    {
        return ImplementOutcome::Failed(format!(
            "tab created but launching the agent failed ({err}) — check the '{}' tab: it may \
             be empty (safe to close) or the agent may have started anyway despite the error, \
             so verify before closing it",
            issue.identifier
        ));
    }

    let mut warnings = Vec::new();

    match client.get_workflow_states(&issue.team.id).await {
        Ok(states) => match plugin::implement::pick_in_progress_state(&states) {
            Some(state) => {
                let updates = json!({ "stateId": state.id });
                if let Err(err) = client.update_issue(&issue.id, updates).await {
                    warnings.push(format!("failed to set state to In Progress: {err}"));
                }
            }
            None => warnings.push("no \"In Progress\"-equivalent workflow state found".to_string()),
        },
        Err(err) => warnings.push(format!("failed to load workflow states: {err}")),
    }

    let prompt = plugin::implement::build_implement_prompt(&issue.identifier);

    // From here on, every early return must still report `warnings` — a failure below doesn't
    // undo (or excuse hiding) a warning collected above it.
    if let Err(err) =
        plugin::herdr_cli::agent_wait(herdr_bin, &created_tab.root_pane_id, "idle", 30_000).await
    {
        return ImplementOutcome::Failed(status_with_warnings(
            format!("agent didn't become ready ({err}) — run manually: {prompt}"),
            &warnings,
        ));
    }

    // Cosmetic only (TF-590's original motivation — avoiding a launch-time name collision —
    // no longer applies, since `pane_run` never passes a name to herdr): best-effort, so a
    // failure here is a warning, not a reason to abandon an otherwise-working flow.
    if let Err(err) =
        plugin::herdr_cli::agent_rename(herdr_bin, &created_tab.root_pane_id, &agent_name).await
    {
        warnings.push(format!("failed to rename the agent pane to {agent_name:?}: {err}"));
    }

    if let Err(err) = send_prompt_until_visible(herdr_bin, &created_tab.root_pane_id, &prompt).await
    {
        return ImplementOutcome::Failed(status_with_warnings(
            format!("{err} — run manually: {prompt}"),
            &warnings,
        ));
    }

    if warnings.is_empty() {
        ImplementOutcome::Started("tab opened, agent started, set to In Progress.".to_string())
    } else {
        ImplementOutcome::StartedWithWarnings(format!("started, but {}", warnings.join("; ")))
    }
}
```

Note what's gone versus the old body: the `shell`/`argv` construction (no `build_shell_argv` call), the `AgentStarted` result and its two guards (TF-579's `started.tab_id != created_tab.tab_id` warning, and the `started.pane_id != created_tab.root_pane_id` redundant-pane-close dance) — none of these apply once the agent runs directly in `created_tab.root_pane_id` via `pane_run` (no split is ever created). `agent_wait`/`send_prompt_until_visible` now target `created_tab.root_pane_id` directly instead of a separately-returned `started.pane_id` (the same pane, just already known one step earlier).

- [ ] **Step 3: Fix `send_prompt_until_visible`'s implementation** to call the renamed `agent_prompt` instead of `agent_send` — find its call to `plugin::herdr_cli::agent_send(...)` and change to `plugin::herdr_cli::agent_prompt(...)` (signature unchanged, so this is a one-word rename at each call site — `grep -n "herdr_cli::agent_send" src/main.rs` to find them all).

- [ ] **Step 4: Update `implement_one`'s doc comment** — remove the sentence "`command` is resolved once per run..." reference to the now-deleted `shell`/`build_shell_argv` construction if present, and update "Any non-fatal warnings collected along the way (the agent landing in an unexpected tab, closing the tab's redundant root pane, ...)" to "Any non-fatal warnings collected along the way (a failed cosmetic agent rename, workflow-state lookup, the actual state transition) are preserved in *every* terminal outcome...".

- [ ] **Step 5: Run tests to verify the build is clean before touching the test suite**

Run: `cargo build --all-targets 2>&1 | grep -E "^error"`
Expected: errors only inside `#[cfg(test)] mod tests` in `main.rs` (Task 7 fixes these) — zero errors in non-test code.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/plugin/implement.rs
git commit -m "fix: redesign implement_one (Implement-on-Enter) for herdr 0.8.0 (TF-624)"
```

---

### Task 7: Rewrite `implement_one`/`implement_many`/prompt-sending tests

**Files:**
- Modify: `src/main.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: everything from Tasks 1-6.
- Produces: nothing new — test-only changes.

- [ ] **Step 1: Replace `write_dispatching_herdr_script`** (currently dispatches `tab create`/`agent start`/`pane close`/`agent wait`) with a version matching the new call set — `tab create`/`pane run`/`agent wait`/`agent rename`:

```rust
/// A `herdr` fake script that dispatches on `$1 $2` so [`implement_one`]'s whole
/// `tab_create` → `pane_run` → `agent_wait` → `agent_rename` → `agent_prompt` sequence can be
/// driven from a single process, each branch supplying its own canned `echo '{...}'; exit N`.
fn write_dispatching_herdr_script(
    tab_create: &str,
    pane_run: &str,
    agent_wait: &str,
    agent_rename: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    write_fake_herdr_script(&format!(
        r#"
case "$1 $2" in
  "tab create") {tab_create} ;;
  "pane run") {pane_run} ;;
  "agent wait") {agent_wait} ;;
  "agent rename") {agent_rename} ;;
  "agent prompt") echo '{{"result":{{}}}}'; exit 0 ;;
  "agent read") echo '{{"result":{{"read":{{"text":""}}}}}}'; exit 0 ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
    ))
}
```

(`agent prompt`/`agent read` get harmless fixed successes here since none of these four tests exercise the prompt-landing poll itself — they all fail before reaching it, same as the pre-existing tests did with `agent send`/`agent read` implicitly unreachable.)

- [ ] **Step 2: Replace `implement_one_fails_immediately_when_tab_create_fails`**:

```rust
#[cfg(unix)]
#[tokio::test]
async fn implement_one_fails_immediately_when_tab_create_fails() {
    let (_dir, script) = write_dispatching_herdr_script(
        r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
        r#"echo '{"error":{"message":"pane run should not run"}}'; exit 1"#,
        r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
        r#"echo '{"error":{"message":"agent rename should not run"}}'; exit 1"#,
    );
    let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
    let issue = sample_issue("TF-579");
    let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

    let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

    let ImplementOutcome::Failed(message) = outcome else {
        panic!("expected Failed, got {outcome:?}");
    };
    assert!(
        message.contains("failed to create a tab") && message.contains("no such workspace"),
        "unexpected message: {message}"
    );
}
```

- [ ] **Step 3: Replace `implement_one_reports_a_possibly_orphaned_tab_when_agent_start_fails`** with `implement_one_reports_a_possibly_orphaned_tab_when_pane_run_fails`:

```rust
#[cfg(unix)]
#[tokio::test]
async fn implement_one_reports_a_possibly_orphaned_tab_when_pane_run_fails() {
    // tab_create succeeds (so a tab now exists), then pane_run fails — the flow must not claim
    // the tab is definitely empty (pane_run's own failure could be a client-side timeout with
    // the agent actually running), and it must not attempt agent_wait/agent_rename afterwards.
    let (_dir, script) = write_dispatching_herdr_script(
        r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
        r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
        r#"echo '{"error":{"message":"agent wait should not run"}}'; exit 1"#,
        r#"echo '{"error":{"message":"agent rename should not run"}}'; exit 1"#,
    );
    let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
    let issue = sample_issue("TF-579");
    let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

    let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

    let ImplementOutcome::Failed(message) = outcome else {
        panic!("expected Failed, got {outcome:?}");
    };
    assert!(
        message.contains("TF-579") && message.contains("no such pane"),
        "unexpected message: {message}"
    );
    assert!(
        !message.contains("an empty"),
        "must not assert the tab is definitely empty: {message}"
    );
}
```

- [ ] **Step 4: Delete `implement_one_records_a_pane_close_failure_as_a_warning_but_continues` and `implement_one_adds_no_warning_when_pane_close_succeeds` entirely** — both test the now-nonexistent redundant-root-pane-close behavior, which no longer exists once `pane_run` runs the agent directly in `created_tab.root_pane_id` (there is no split, so nothing is ever closed).

- [ ] **Step 5: Add a replacement test covering the new `agent_rename` warning path**, in the same spot:

```rust
#[cfg(unix)]
#[tokio::test]
async fn implement_one_records_an_agent_rename_failure_as_a_warning_but_continues() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/graphql")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({"data": null, "errors": [{"message": "workflow states unavailable"}]})
                .to_string(),
        )
        .create_async()
        .await;
    let client = herdr_linear::LinearClient::with_endpoint(
        "lin_api_test",
        format!("{}/graphql", server.url()),
    )
    .unwrap();
    let (_dir, script) = write_dispatching_herdr_script(
        r#"echo '{"result":{"tab":{"tab_id":"t2","label":"TF-579"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
        r#"echo '{"result":{}}'; exit 0"#,
        r#"echo '{"result":{}}'; exit 0"#,
        r#"echo '{"error":{"message":"agent_not_found"}}'; exit 1"#,
    );
    let issue = sample_issue("TF-579");
    let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

    let outcome = implement_one(script.to_str().unwrap(), &client, &issue, &command).await;

    let ImplementOutcome::StartedWithWarnings(message) = outcome else {
        panic!("expected StartedWithWarnings, got {outcome:?}");
    };
    assert!(
        message.contains("failed to rename the agent pane") && message.contains("agent_not_found"),
        "agent_rename failure warning missing: {message}"
    );
    assert!(
        message.contains("failed to load workflow states"),
        "unexpected message: {message}"
    );
}
```

- [ ] **Step 6: Update `write_batch_concurrency_probe_script`** — move the intentional-failure injection from the (now-removed) `agent start` branch to `tab create` itself, echoing back the `--label` value (the per-issue `issue.identifier`, always `$6` in `tab create --cwd <cwd> --label <label> --focus`) instead of the old agent-name echo, so the pairing assertion in `implement_many_runs_issues_concurrently_up_to_the_default_batch_limit` still has a per-issue-unique value to check:

```rust
fn write_batch_concurrency_probe_script(
    delay: std::time::Duration,
) -> (tempfile::TempDir, std::path::PathBuf) {
    write_fake_herdr_script(&format!(
        r#"case "$1 $2" in
  "agent list")
    echo '{{"result":{{"agents":[{{"agent":"claude"}}]}}}}'
    exit 0
    ;;
  "tab create")
    script_dir=$(dirname "$0")
    mkdir -p "$script_dir/inflight" "$script_dir/peaks"
    : > "$script_dir/inflight/$$"
    count=$(ls "$script_dir/inflight" | wc -l | tr -d ' ')
    echo "$count" > "$script_dir/peaks/$$"
    sleep {delay_secs}
    rm -f "$script_dir/inflight/$$"
    echo "{{\"error\":{{\"message\":\"tab create intentionally fails for the concurrency probe (label: $6)\"}}}}"
    exit 1
    ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#,
        delay_secs = delay.as_secs_f64()
    ))
}
```

- [ ] **Step 7: Update `implement_many_runs_issues_concurrently_up_to_the_default_batch_limit`'s assertions** to match — failure message substring changes from `"agent start intentionally fails"` to `"tab create intentionally fails"`, and the pairing check switches from `build_agent_name` (no longer echoed anywhere in this failure path) to the raw `issue.identifier` (which `tab create --label` receives directly):

```rust
#[cfg(unix)]
#[tokio::test]
async fn implement_many_runs_issues_concurrently_up_to_the_default_batch_limit() {
    const DEFAULT_BATCH_CONCURRENCY: usize = 5;
    const ISSUE_COUNT: usize = 2 * DEFAULT_BATCH_CONCURRENCY;
    let delay = std::time::Duration::from_millis(300);

    let (dir, script) = write_batch_concurrency_probe_script(delay);
    let client = herdr_linear::LinearClient::new("lin_api_test_key").unwrap();
    let issues: Vec<_> = (0..ISSUE_COUNT)
        .map(|i| sample_issue(&format!("TF-{i}")))
        .collect();
    let command = plugin::implement::ValidatedAgentCommand::parse("hr".to_string()).unwrap();

    let started = std::time::Instant::now();
    let results = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        implement_many(script.to_str().unwrap(), &client, issues, &command),
    )
    .await
    .expect("implement_many hung");
    let elapsed = started.elapsed();

    assert_eq!(results.len(), ISSUE_COUNT);
    for (identifier, outcome) in &results {
        let ImplementOutcome::Failed(message) = outcome else {
            panic!("expected every issue to fail (tab create always fails): {identifier} -> {outcome:?}");
        };
        assert!(
            message.contains("tab create intentionally fails"),
            "unexpected failure for {identifier}: {message}"
        );
        // Pairing check: the fake `tab create` echoed back its own `--label` value, which is
        // this same `identifier` — if a future change shuffled identifiers against the wrong
        // outcome, this issue's own identifier wouldn't appear in its own message.
        assert!(
            message.contains(identifier),
            "outcome for {identifier} doesn't carry its own identifier — got: {message} \
             (identifier/outcome pairing may be broken)"
        );
    }

    let peak = peak_concurrency(&dir);
    assert!(
        peak > 1,
        "peak concurrent `tab create` calls was {peak} — issues are running sequentially, \
         not through execute_batch"
    );
    assert!(
        peak <= DEFAULT_BATCH_CONCURRENCY,
        "peak concurrent `tab create` calls was {peak}, above the documented default \
         concurrency cap of {DEFAULT_BATCH_CONCURRENCY}"
    );
    assert!(
        elapsed >= delay,
        "completed in {elapsed:?}, faster than a single `tab create` delay of {delay:?} \
         (too fast — every issue should wait out at least one delay)"
    );
}
```

Update the test's preceding doc comment's sentence "the fake `agent start` above echoes the agent name it was invoked with back into its failure message, and `build_agent_name` derives that name..." to "the fake `tab create` above echoes its own `--label` argument (the issue's identifier) back into its failure message...".

- [ ] **Step 8: Rename `"agent send"` to `"agent prompt"` in the three remaining prompt-sending fixture scripts** — `write_prompt_send_read_sequence_script`, `write_prompt_send_lands_on_attempt_script`, `write_prompt_read_always_script` — each has exactly one `"agent send")` case label; change it to `"agent prompt")` in all three (the branch bodies are unchanged, only the dispatch label). These back the six `send_prompt_until_visible_*`/`wait_for_prompt_stable_*` tests (lines ~2585-2745) — none of their own bodies need changes beyond this, since they only ever call `send_prompt_until_visible`/`wait_for_prompt_stable` directly (not `implement_one`), and Task 6 Step 3 already updated `send_prompt_until_visible`'s internal call from `agent_send` to `agent_prompt`.

- [ ] **Step 9: Run the full test suite**

Run: `cargo test`
Expected: all tests pass, in particular every `implement_one_*`, `implement_many_*`, `open_config_*`, `send_prompt_until_visible_*`, `wait_for_prompt_stable_*`.

- [ ] **Step 10: Run clippy and fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 11: Commit**

```bash
git add src/main.rs
git commit -m "test: rewrite implement_one/implement_many/prompt fixtures for herdr 0.8.0 (TF-624)"
```

---

### Task 8: CHANGELOG, README, and `min_herdr_version` compatibility update

**Why this task exists:** once the plugin works again, the *actual* herdr version it now requires must be written down somewhere a user hits before they hit the failure — not just fixed in code. This plugin has only ever been verified against herdr 0.8.0 (the version installed during TF-624's investigation); nothing lower than that has been tested against the new `pane_run`/`tab_list`/`tab_focus`/`agent_rename`/`agent_prompt`/`agent_wait --until` calls this plan introduces, so the honest floor to publish is `0.8.0`, not a guessed-at earlier version.

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `herdr-plugin.toml`
- Modify: `src/plugin/herdr_cli.rs`
- Modify: `README.md`

**Interfaces:** none — documentation and a version-constant bump only.

- [ ] **Step 1: Bump `min_herdr_version` in `herdr-plugin.toml`** from `"0.7.0"` to `"0.8.0"`:

```toml
min_herdr_version = "0.8.0"
```

- [ ] **Step 2: Bump `MIN_HERDR_VERSION` in `src/plugin/herdr_cli.rs`** to match — find:

```rust
const MIN_HERDR_VERSION: &str = "0.7.0";
```

and change to:

```rust
const MIN_HERDR_VERSION: &str = "0.8.0";
```

The doc comment directly above it already says "mirroring `min_herdr_version` in `herdr-plugin.toml`" — leave that wording as-is, it's still accurate. The existing test `interpret_output_hints_at_upgrading_herdr_when_cwd_flag_is_unsupported` (Task 3) asserts against the `MIN_HERDR_VERSION` constant symbolically (`err.contains(MIN_HERDR_VERSION)`), not a literal `"0.7.0"` string, so it needs no separate edit here.

- [ ] **Step 3: Update the one literal version-floor mention in `README.md`** (around line 449, inside the `<Enter>`-launch-context `[!NOTE]` block) — change:

```markdown
> the **split** action (`herdr-linear.open-split`) or the **tab** one
> (`herdr-linear.open-tab`). This requires herdr ≥ 0.7.0 (see `min_herdr_version` in
> `herdr-plugin.toml`); on an older/misbehaving herdr that omits the launch context,
```

to:

```markdown
> the **split** action (`herdr-linear.open-split`) or the **tab** one
> (`herdr-linear.open-tab`). This requires herdr ≥ 0.8.0 (see `min_herdr_version` in
> `herdr-plugin.toml`); on an older/misbehaving herdr that omits the launch context,
```

- [ ] **Step 4: Add a `### Requirements` subsection to `README.md`**, immediately after the `## Herdr Plugin` section's opening paragraph and before the screenshot `<table>` (i.e. right after the line ending "...pressing `<Enter>` on a selected issue is not — see \"Use\" below." and before `<table>`):

```markdown
### Requirements

Requires **herdr >= 0.8.0** (see `min_herdr_version` in `herdr-plugin.toml`). herdr's own
`agent`/`pane`/`tab` CLI surface has changed shape between releases before (TF-604, TF-624) —
this plugin has only ever been verified against 0.8.0; an older installed herdr will fail with
`herdr config check`-style "unknown option"/"unknown subcommand" errors rather than a plugin bug.
Run `herdr --version` to check yours, and `herdr update` to upgrade.

```

- [ ] **Step 5: Add a `### Fixed` entry under `[Unreleased]`** in `CHANGELOG.md` (above the existing TF-623 benchmark-suite entry, keeping newest-first order):

```markdown
- `c` (open `config.toml`) and Implement-on-Enter both silently failed against herdr >= 0.8.0,
  which redesigned `agent start`/`agent wait`/`agent send` out from under this plugin: `agent
  start` dropped `--cwd`/`--tab`/`--focus` + arbitrary argv in favor of `--kind`/`--pane` against
  a fixed enum of recognized agent binaries (unable to launch `nvim` or a custom `agent_command`
  wrapper alias like `"hr"`), `agent wait` renamed `--status` to `--until`, and `agent send` was
  replaced by `agent prompt`. Both flows now open their tab via `tab_create` (unchanged) and type
  the launch command into its root pane via a new `pane_run` wrapper instead — herdr's own
  passive auto-detection picks up whatever recognized agent ends up running, same as before.
  TF-604's "upgrade herdr" hint (below) was addressing a different, no-longer-applicable case;
  see TF-624 for the actual current-herdr incompatibility and its fix (TF-624)

- TF-604's `--cwd`-rejection hint assumed the *only* way an installed herdr could reject `--cwd`
  on `agent start`/`tab create` was predating `min_herdr_version = 0.7.0`. That's no longer true
  for `agent start`: herdr >= 0.8.0 (well above the floor) rejects it too, having redesigned the
  subcommand's flags entirely (see TF-624) — the hint's wording is now only accurate for
  `tab_create`, the one remaining `--cwd`-accepting call (TF-624)

- `min_herdr_version` (in `herdr-plugin.toml`, mirrored by `MIN_HERDR_VERSION` in
  `herdr_cli.rs`) raised `0.7.0` → `0.8.0`: the new `pane_run`/`tab_list`/`tab_focus`/
  `agent_rename`/`agent_prompt`/`agent_wait --until` calls this fix introduces have only ever
  been verified against herdr 0.8.0 — publishing the old, now-inaccurate `0.7.0` floor would
  send users on an older herdr into the exact silent-failure this ticket exists to fix. See the
  new "Requirements" section in `README.md` (TF-624)
```

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md herdr-plugin.toml src/plugin/herdr_cli.rs README.md
git commit -m "docs: changelog + README compatibility note for the herdr 0.8.0 agent-CLI redesign (TF-624)

Also raises min_herdr_version 0.7.0 -> 0.8.0 (herdr_cli.rs's MIN_HERDR_VERSION mirrors it) —
the new pane_run/tab_list/tab_focus/agent_rename/agent_prompt/agent_wait --until calls this
fix introduces have only ever been verified against 0.8.0."
```

---

## Self-Review

- **Spec coverage:** `c` keybinding (Task 5), Implement-on-Enter (Task 6-7), `agent_wait`/`agent_prompt` flag fixes (Task 2), dead-code removal (Task 3), CHANGELOG/README/`min_herdr_version` compatibility documentation (Task 8) — all of TF-624's "Fix direction" bullets, plus the follow-up requirement to document the verified-compatible herdr version once the fix lands, are covered.
- **Placeholder scan:** every step carries literal code, literal test bodies, or an exact line-range/grep target — no "similar to above" or "add appropriate handling" language.
- **Type consistency:** `pane_run`/`tab_list`/`tab_focus`/`agent_rename`/`agent_prompt` signatures introduced in Tasks 1-2 are used identically (same names, same argument order) in Tasks 5-7.
