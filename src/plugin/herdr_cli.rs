//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `implement_one`, shared by both the single- and
//! multi-issue callers) to open a tab, type the launch command into it, wait for the resulting
//! agent to become ready, and inject text — and by the `c` keybinding's
//! `open_config_in_herdr_pane` to open/reuse a config-editor tab the same way. The
//! subprocess-spawning half is deliberately untested at this layer — same status as the existing
//! `open::that(url)` call for the `o` key; see
//! docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for why. The
//! response-interpretation half (`interpret_output`) is pure and unit-tested below.
//!
//! ## `agent_wait`'s retry-on-missing-`result` workaround
//!
//! herdr v0.7.3 has a reproducible bug: `herdr agent wait --status idle`'s underlying
//! `events.subscribe` stream closes as soon as the target pane's *agent identity* is first
//! detected (e.g. `previous_agent=None → agent=Some(Claude)`, logged the moment herdr notices
//! which CLI is running in the pane), not when its *status* actually reaches the requested
//! value — confirmed via `herdr-server.log` on every observed `agent.start` call, each followed
//! within ~200ms by `outcome="stream_closed"`, long before the agent could plausibly be idle.
//! The CLI then exits 0 with valid JSON missing the `result` field its own schema (`herdr api
//! schema --json`) declares `required`. `agent_wait` retries specifically on this response (see
//! `is_missing_result_response`) — the identity-transition event fires at most once per pane,
//! so a retried call reliably observes the real status change instead. Revisit/remove once
//! herdr fixes this upstream.

use crate::error::Error;
use crate::Result;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// A herdr pane identifier (e.g. `"wY:p3"`). A newtype rather than a bare `String` so it can't
/// be swapped for a [`TabId`] at a call site without a compile error — the two identifier
/// spaces are not interchangeable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneId(String);

impl PaneId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A herdr tab identifier (e.g. `"wY:tW"`). See [`PaneId`] for why this is a newtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabId(String);

impl TabId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Result of a successful `herdr tab create` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCreated {
    pub tab_id: TabId,
    pub root_pane_id: PaneId,
}

/// Resolve the `herdr` binary path: `$HERDR_BIN_PATH`, falling back to `"herdr"` on `$PATH` —
/// the same convention `scripts/open-tab.sh` uses.
pub fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

/// herdr's minimum required version for this plugin, mirroring `min_herdr_version` in
/// `herdr-plugin.toml` (duplicated here because that manifest field isn't readable by code in
/// this crate at compile time — keep the two in sync by hand if either changes). Used only to
/// word [`unsupported_cwd_flag_hint`]'s upgrade hint.
const MIN_HERDR_VERSION: &str = "0.7.0";

/// If `message` is herdr's own CLI-parser rejection of the `--cwd` flag, returns an upgrade hint
/// to append to it. The only `herdr` call this plugin makes to a subcommand that accepts `--cwd`
/// ([`tab_create`]) passes it unconditionally, so the only way an installed
/// `herdr` binary could reject it is if that binary predates the version which added `--cwd`
/// support (TF-579, TF-584) — i.e. it's older than [`MIN_HERDR_VERSION`]. Without this hint, the
/// raw "unknown option: --cwd" that reaches the user (TF-604) gives no indication that the fix is
/// upgrading herdr rather than anything on the plugin side. Matches only herdr's exact observed
/// wording — a substring check, not a general "any unknown option" detector — so an unrelated
/// unknown-option failure (e.g. a typo introduced elsewhere) isn't misattributed to this cause.
fn unsupported_cwd_flag_hint(message: &str) -> Option<String> {
    if message.contains("unknown option: --cwd") {
        Some(format!(
            "this herdr installation doesn't support --cwd on this subcommand; herdr-linear \
             requires herdr >= {MIN_HERDR_VERSION} (see min_herdr_version in herdr-plugin.toml) — \
             upgrade herdr and retry"
        ))
    } else {
        None
    }
}

/// Pure interpretation of a `herdr` CLI invocation's raw output into the `Result` `run` returns.
/// Maps a non-zero exit, a top-level `{"error": {"message": ...}}` response (checked
/// independently of the exit code — a future protocol change that reports failure via body
/// alone, exit 0, must not be misread as success), or unparseable JSON to `Error::Internal` with
/// the CLI's own error message (or raw stderr/stdout as a fallback) so failures are always
/// actionable in the status banner they end up in. One specific failure gets a further hint
/// appended — see [`unsupported_cwd_flag_hint`] — an installed `herdr` too old to support `--cwd`
/// on `tab create` (TF-604). Split out from `run` so this logic — the part that
/// actually decides success vs. failure — is unit-testable without spawning a process.
fn interpret_output(
    command_desc: &str,
    status_success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Value> {
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

/// True if `error` is herdr's known "no `result` field" response — see the module docs for the
/// underlying stream-close race this works around. Distinguishes this one retryable case from a
/// genuine failure (a real timeout, no such pane, an actual protocol error), so `agent_wait`
/// only retries the failure mode retrying is likely to fix. Matches on the dedicated
/// [`Error::MissingResultField`] variant rather than a substring of the formatted message, so a
/// future wording change in [`interpret_output`] can't silently disable the retry.
fn is_missing_result_response(error: &Error) -> bool {
    matches!(error, Error::MissingResultField(_))
}

/// Wall-clock ceiling for `herdr` subprocess calls that don't carry their own `--timeout`
/// argument (everything routed through [`run`]: `agent_list`, `tab_create`, `agent_prompt`,
/// `pane_close`, `pane_run`, `tab_list`, `tab_focus`, `agent_rename`). Without this, a hung
/// `herdr` daemon blocks the single-threaded TUI's event loop indefinitely — `agent_wait` is the
/// exception, since it computes its own call-specific bound in [`agent_wait`] instead of using
/// this constant.
const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Max attempts and per-attempt backoff for [`spawn_with_etxtbsy_retry`]'s retry loop —
/// `ETXTBSY` always resolves on its own once whatever briefly held the executable open
/// for writing lets go, so a handful of short retries reliably rides out the window
/// without meaningfully affecting real (non-`ETXTBSY`) latency.
const ETXTBSY_MAX_RETRIES: u32 = 5;
const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Spawns `herdr_bin args`, retrying up to [`ETXTBSY_MAX_RETRIES`] times with a short
/// delay if the OS reports `ErrorKind::ExecutableFileBusy` ("text file busy" / `ETXTBSY`)
/// — a transient condition (something else has the executable open for writing at the
/// exact instant of `execve`), not a real, persistent failure. Most reliably hit by this
/// crate's own tests, which write and `chmod` a fresh fake `herdr` script then exec it
/// almost immediately (a known kernel/VFS race on some CI filesystems) — but the same
/// condition could in principle hit a real `herdr` binary that's mid-reinstall/update at
/// the exact moment this runs, so the retry lives here rather than only in test scaffolding.
async fn spawn_with_etxtbsy_retry(
    herdr_bin: &str,
    args: &[&str],
) -> std::io::Result<std::process::Output> {
    let mut attempt = 0;
    loop {
        match Command::new(herdr_bin).args(args).output().await {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempt < ETXTBSY_MAX_RETRIES =>
            {
                attempt += 1;
                tokio::time::sleep(ETXTBSY_RETRY_DELAY).await;
            }
            result => return result,
        }
    }
}

/// Run a `herdr` CLI subcommand, bounded by `call_timeout`, returning the parsed `result` field
/// on success. See [`interpret_output`] for the success/failure mapping.
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

/// [`run_with_timeout`] bounded by [`DEFAULT_CLI_TIMEOUT`] — used by every `herdr` subcommand
/// except `agent_wait`, which computes its own call-specific budget instead (see [`agent_wait`]).
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(herdr_bin, args, DEFAULT_CLI_TIMEOUT).await
}

/// `herdr agent list` — the raw JSON text of the `result` field, for
/// [`crate::plugin::implement::resolve_preferred_agent`] to parse.
pub async fn agent_list(herdr_bin: &str) -> Result<String> {
    let result = run(herdr_bin, &["agent", "list"]).await?;
    Ok(result.to_string())
}

/// Extract [`TabCreated`] from a `herdr tab create` call's already-unwrapped `result` value.
/// Split out from [`tab_create`] for the same testability reason [`interpret_output`] is split
/// out of `run`.
fn parse_tab_created(result: &Value) -> Result<TabCreated> {
    let tab_id = result
        .get("tab")
        .and_then(|t| t.get("tab_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Internal("tab.create response missing tab.tab_id".to_string()))?
        .to_string();
    let root_pane_id = result
        .get("root_pane")
        .and_then(|p| p.get("pane_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Error::Internal("tab.create response missing root_pane.pane_id".to_string())
        })?
        .to_string();

    Ok(TabCreated {
        tab_id: TabId(tab_id),
        root_pane_id: PaneId(root_pane_id),
    })
}

/// `herdr tab create --cwd <cwd> --label <label> --focus` — creates a fresh, focused tab that is
/// already labeled `label`, and returns its [`TabCreated`] (the new tab's id, plus the id of the
/// single root pane herdr creates inside it by default). Labeling at creation time (rather than
/// via a follow-up `tab rename`) means the label is correct from the very first frame, with no
/// window in which the tab could be confused with — or have its label stolen by — a different,
/// already-running tab. `root_pane_id` exists to be closed once [`agent_start`] has placed the
/// real agent pane into this tab — `agent_start` never replaces or consumes a tab's existing
/// panes, it only adds a split alongside them, so without closing it every tab would carry a
/// permanent extra empty shell pane. See
/// docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for why this replaced a
/// rename-after-`agent_start` sequence.
pub async fn tab_create(herdr_bin: &str, cwd: &Path, label: &str) -> Result<TabCreated> {
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

/// `herdr pane close <pane_id>`. Used to close the now-redundant root pane [`tab_create`] leaves
/// behind once [`agent_start`] has placed the real agent pane into the same tab (`agent_start`
/// never replaces a tab's existing panes, only splits alongside them).
pub async fn pane_close(herdr_bin: &str, pane_id: &PaneId) -> Result<()> {
    run(herdr_bin, &["pane", "close", pane_id.as_str()])
        .await
        .map(|_| ())
}

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

/// Convenience wrapper around [`find_tab_id_by_label`] for [`crate::open_config_in_herdr_pane`]:
/// looks for an existing tab labeled [`crate::plugin::editor::EDITOR_AGENT_NAME`] in a `tab
/// list` JSON result.
pub fn find_existing_editor_tab(tab_list_json: &str) -> Option<TabId> {
    find_tab_id_by_label(tab_list_json, crate::plugin::editor::EDITOR_AGENT_NAME)
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

/// Extra attempts `agent_wait` makes when herdr responds with the missing-`result` bug (see the
/// module docs) before giving up and returning that error to the caller.
const AGENT_WAIT_MAX_RETRIES: u32 = 2;

/// Buffer added on top of the caller's remaining `--timeout` budget when bounding each
/// `herdr agent wait` subprocess call, so the outer wall-clock guard ([`run_with_timeout`])
/// doesn't race herdr's own internal timeout — herdr should always get the chance to return its
/// own clean timeout response first.
const AGENT_WAIT_CALL_TIMEOUT_BUFFER: Duration = Duration::from_secs(5);

/// Decide whether `agent_wait`'s loop should retry after `err`, and if so, the timeout budget
/// (in ms) remaining for the next attempt. Pure — no I/O — so the retry decision is
/// unit-testable without mocking the `herdr` subprocess (the loop itself previously wasn't).
/// Returns `None` (stop, return `err` to the caller) when `err` isn't the known retryable case,
/// the retry budget ([`AGENT_WAIT_MAX_RETRIES`]) is exhausted, or there's no time left in the
/// caller's budget to retry with — previously the loop clamped a fully-consumed budget to 1ms
/// and fired one more subprocess call anyway, silently overrunning the caller's timeout.
fn next_retry_budget_ms(
    err: &Error,
    attempt: u32,
    elapsed_ms: u64,
    timeout_ms: u64,
) -> Option<u64> {
    if attempt >= AGENT_WAIT_MAX_RETRIES || !is_missing_result_response(err) {
        return None;
    }
    let remaining_ms = timeout_ms.saturating_sub(elapsed_ms);
    if remaining_ms == 0 {
        return None;
    }
    Some(remaining_ms)
}

/// `herdr agent wait <pane_id> --until <status> --timeout <timeout_ms>` (herdr 0.8.0 renamed `--status` to `--until`, TF-624). Retries, within the
/// caller's original `timeout_ms` budget, when herdr responds with the missing-`result` bug
/// documented at the top of this module — any other error (a real timeout, no such pane, ...)
/// returns immediately.
pub async fn agent_wait(
    herdr_bin: &str,
    pane_id: &PaneId,
    status: &str,
    timeout_ms: u64,
) -> Result<()> {
    let start = std::time::Instant::now();
    let mut attempt = 0;
    let mut remaining_ms = timeout_ms;

    loop {
        let timeout_str = remaining_ms.to_string();
        let call_timeout = Duration::from_millis(remaining_ms) + AGENT_WAIT_CALL_TIMEOUT_BUFFER;

        let result = run_with_timeout(
            herdr_bin,
            &[
                "agent",
                "wait",
                pane_id.as_str(),
                "--until",
                status,
                "--timeout",
                &timeout_str,
            ],
            call_timeout,
        )
        .await;

        let err = match result {
            Ok(_) => return Ok(()),
            Err(err) => err,
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;
        match next_retry_budget_ms(&err, attempt, elapsed_ms, timeout_ms) {
            Some(next_remaining_ms) => {
                attempt += 1;
                remaining_ms = next_remaining_ms;
            }
            None => return Err(err),
        }
    }
}

/// `herdr agent prompt <pane_id> <text>` (herdr 0.8.0 replaced the old `agent send` subcommand
/// with `agent prompt`, which additionally supports `--wait`/`--until`/`--timeout` options this
/// plugin doesn't need — it does its own stability polling via `agent_read` instead, see
/// `main.rs`'s `send_prompt_until_visible`).
pub async fn agent_prompt(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "prompt", pane_id.as_str(), text])
        .await
        .map(|_| ())
}

/// Extract the rendered pane text from a `herdr agent read` call's already-unwrapped `result`
/// value. Split out from [`agent_read`] for the same testability reason [`interpret_output`] is
/// split out of `run`.
fn parse_agent_read(result: &Value) -> Result<String> {
    result
        .get("read")
        .and_then(|r| r.get("text"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| Error::Internal("agent.read response missing read.text".to_string()))
}

/// `herdr agent read <pane_id> --source <source> --lines <lines>` — the pane's rendered
/// terminal text. Used by `main.rs`'s `send_prompt_until_visible` to confirm an [`agent_prompt`]
/// actually reached the target's input box, rather than trusting `agent_wait`'s screen-scraped
/// "idle" status alone: that status can go true the instant the prompt box is *painted*, which
/// can be a beat before the target's input loop has actually attached to read the pty — a
/// keystroke written into that gap is silently dropped, not queued (see the module docs above
/// for the related, already-worked-around `agent_wait` race).
pub async fn agent_read(
    herdr_bin: &str,
    pane_id: &PaneId,
    source: &str,
    lines: u32,
) -> Result<String> {
    let lines_str = lines.to_string();
    let result = run(
        herdr_bin,
        &[
            "agent",
            "read",
            pane_id.as_str(),
            "--source",
            source,
            "--lines",
            &lines_str,
        ],
    )
    .await?;
    parse_agent_read(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpret_output_returns_the_result_field_on_success() {
        let result = interpret_output("herdr agent list", true, r#"{"result":{"agents":[]}}"#, "");

        assert_eq!(result.unwrap(), serde_json::json!({"agents": []}));
    }

    #[test]
    fn interpret_output_errors_on_non_zero_exit_with_a_json_error_body() {
        let result = interpret_output(
            "herdr agent send bogus hi",
            false,
            r#"{"error":{"message":"no such pane"}}"#,
            "",
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no such pane"), "unexpected message: {err}");
    }

    #[test]
    fn interpret_output_errors_on_non_zero_exit_with_no_json_body_falling_back_to_stderr() {
        let result = interpret_output(
            "herdr agent wait bogus",
            false,
            "",
            "timed out waiting for idle\n",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out waiting for idle"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_errors_on_non_zero_exit_with_no_stderr_falling_back_to_stdout() {
        let result = interpret_output("herdr bogus", false, "unknown option: --bogus\n", "");

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown option: --bogus"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_hints_at_upgrading_herdr_when_cwd_flag_is_unsupported() {
        // TF-604: an installed `herdr` binary older than the version that added `--cwd` support
        // to `tab create` rejects it with this exact wording. The raw message alone
        // (as exercised by the `--bogus` case above) leaves the user with no idea *why* — this
        // hint should point at the actual fix (upgrade herdr) instead of just echoing herdr's
        // own CLI-parser error back at them.
        let result = interpret_output(
            "herdr tab create --cwd /repo --label TF-604 --focus",
            false,
            "",
            "unknown option: --cwd\n",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown option: --cwd") && err.contains(MIN_HERDR_VERSION),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_errors_on_a_json_error_body_even_with_a_zero_exit() {
        // A hypothetical protocol variant that reports failure via the body alone must not be
        // misread as success just because the process exited 0.
        let result = interpret_output(
            "herdr agent send bogus hi",
            true,
            r#"{"error":{"message":"no such pane"}}"#,
            "",
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no such pane"), "unexpected message: {err}");
    }

    #[test]
    fn interpret_output_errors_on_unparseable_json_with_a_zero_exit() {
        let result = interpret_output("herdr agent list", true, "not json", "");

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unparseable output"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_errors_when_the_result_field_is_missing() {
        let result = interpret_output("herdr agent list", true, r#"{"id":"cli:agent:list"}"#, "");

        assert!(result.is_err());
    }

    #[test]
    fn is_missing_result_response_matches_the_missing_result_error() {
        let err = interpret_output(
            "herdr agent wait wY:p1Q --status idle",
            true,
            r#"{"id":"x"}"#,
            "",
        )
        .unwrap_err();

        assert!(is_missing_result_response(&err));
    }

    #[test]
    fn is_missing_result_response_does_not_match_other_internal_errors() {
        let failed = interpret_output(
            "herdr agent wait bogus",
            false,
            r#"{"error":{"message":"no such pane"}}"#,
            "",
        )
        .unwrap_err();
        let unparseable = interpret_output("herdr agent list", true, "not json", "").unwrap_err();
        let spawn_failed = Error::Internal("Failed to run `herdr`: no such file".to_string());

        assert!(!is_missing_result_response(&failed));
        assert!(!is_missing_result_response(&unparseable));
        assert!(!is_missing_result_response(&spawn_failed));
    }

    /// Writes an executable fake `herdr` shell script (`#!/bin/sh` — Unix only, matching the
    /// project's own `sh -i`/`/bin/bash` assumptions) into a fresh [`tempfile::TempDir`] and
    /// returns both, so the subprocess-spawning `herdr_cli` functions below (`tab_create`,
    /// `pane_close`, ...) can be exercised end-to-end without a real `herdr` daemon. Each such
    /// function takes `herdr_bin` directly as a parameter (see `herdr_bin()`'s
    /// `$HERDR_BIN_PATH` override, which this mirrors), so the script's path can be passed
    /// straight in — no env var indirection needed. The `TempDir` must be kept alive by the
    /// caller for the duration of the test (dropping it deletes the script).
    #[cfg(unix)]
    fn write_fake_herdr_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("herdr");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, script)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tab_create_builds_the_expected_cli_invocation_and_parses_its_response() {
        let capture_dir = tempfile::tempdir().unwrap();
        let args_file = capture_dir.path().join("args.txt");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
printf '%s\n' "$@" > "{}"
echo '{{"result":{{"tab":{{"tab_id":"t2","label":"TF-579"}},"root_pane":{{"pane_id":"p9"}}}}}}'
exit 0
"#,
            args_file.display()
        ));

        let created = tab_create(script.to_str().unwrap(), Path::new("/tmp"), "TF-579")
            .await
            .expect("tab_create should succeed");

        assert_eq!(created.tab_id.as_str(), "t2");
        assert_eq!(created.root_pane_id.as_str(), "p9");

        let captured = std::fs::read_to_string(&args_file).unwrap();
        let args: Vec<&str> = captured.lines().collect();
        assert_eq!(
            args,
            vec!["tab", "create", "--cwd", "/tmp", "--label", "TF-579", "--focus"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tab_create_propagates_a_herdr_error() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"message":"no such workspace"}}'
exit 1
"#,
        );

        let err = tab_create(script.to_str().unwrap(), Path::new("/tmp"), "TF-579")
            .await
            .expect_err("tab_create should propagate the herdr error");

        assert!(
            err.to_string().contains("no such workspace"),
            "unexpected message: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pane_close_invokes_the_expected_cli_command() {
        let capture_dir = tempfile::tempdir().unwrap();
        let args_file = capture_dir.path().join("args.txt");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
printf '%s\n' "$@" > "{}"
echo '{{"result":{{}}}}'
exit 0
"#,
            args_file.display()
        ));

        pane_close(script.to_str().unwrap(), &PaneId("p9".to_string()))
            .await
            .expect("pane_close should succeed");

        let captured = std::fs::read_to_string(&args_file).unwrap();
        let args: Vec<&str> = captured.lines().collect();
        assert_eq!(args, vec!["pane", "close", "p9"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pane_close_propagates_a_herdr_error() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"message":"no such pane"}}'
exit 1
"#,
        );

        let err = pane_close(script.to_str().unwrap(), &PaneId("p9".to_string()))
            .await
            .expect_err("pane_close should propagate the herdr error");

        assert!(
            err.to_string().contains("no such pane"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn parse_tab_created_extracts_the_tab_id_and_root_pane_id() {
        let result = serde_json::json!({
            "tab": {"tab_id": "wY:t2D", "label": "TF-579"},
            "root_pane": {"pane_id": "wY:p31"}
        });

        let created = parse_tab_created(&result).unwrap();

        assert_eq!(created.tab_id.as_str(), "wY:t2D");
        assert_eq!(created.root_pane_id.as_str(), "wY:p31");
    }

    #[test]
    fn parse_tab_created_errors_when_tab_id_is_missing() {
        let result = serde_json::json!({
            "tab": {"label": "TF-579"},
            "root_pane": {"pane_id": "wY:p31"}
        });

        let err = parse_tab_created(&result).unwrap_err().to_string();

        assert!(err.contains("tab.tab_id"), "unexpected message: {err}");
    }

    #[test]
    fn parse_tab_created_errors_when_the_tab_object_is_missing_entirely() {
        let result = serde_json::json!({"root_pane": {"pane_id": "wY:p31"}});

        assert!(parse_tab_created(&result).is_err());
    }

    #[test]
    fn parse_tab_created_errors_when_root_pane_id_is_missing() {
        let result = serde_json::json!({
            "tab": {"tab_id": "wY:t2D", "label": "TF-579"},
            "root_pane": {}
        });

        let err = parse_tab_created(&result).unwrap_err().to_string();

        assert!(
            err.contains("root_pane.pane_id"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn parse_tab_created_errors_when_the_root_pane_object_is_missing_entirely() {
        let result = serde_json::json!({"tab": {"tab_id": "wY:t2D", "label": "TF-579"}});

        let err = parse_tab_created(&result).unwrap_err().to_string();

        assert!(
            err.contains("root_pane.pane_id"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn parse_agent_read_extracts_the_rendered_text() {
        let result = serde_json::json!({"read": {"text": "❯ Implement Linear Issue TF-579"}});

        let text = parse_agent_read(&result).unwrap();

        assert_eq!(text, "❯ Implement Linear Issue TF-579");
    }

    #[test]
    fn parse_agent_read_errors_when_read_text_is_missing() {
        let result = serde_json::json!({"read": {"pane_id": "wY:p10"}});

        let err = parse_agent_read(&result).unwrap_err().to_string();

        assert!(err.contains("read.text"), "unexpected message: {err}");
    }

    #[test]
    fn parse_agent_read_errors_when_the_read_object_is_missing_entirely() {
        let result = serde_json::json!({"id": "cli:agent:read"});

        assert!(parse_agent_read(&result).is_err());
    }

    #[test]
    fn next_retry_budget_ms_retries_a_missing_result_response_within_budget() {
        let err = Error::MissingResultField("`herdr agent wait` had no `result` field".to_string());

        assert_eq!(next_retry_budget_ms(&err, 0, 5_000, 30_000), Some(25_000));
    }

    #[test]
    fn next_retry_budget_ms_stops_once_the_retry_cap_is_reached() {
        let err = Error::MissingResultField("`herdr agent wait` had no `result` field".to_string());

        assert_eq!(
            next_retry_budget_ms(&err, AGENT_WAIT_MAX_RETRIES, 5_000, 30_000),
            None
        );
    }

    #[test]
    fn next_retry_budget_ms_does_not_retry_a_non_retryable_error() {
        let err = Error::Internal("no such pane".to_string());

        assert_eq!(next_retry_budget_ms(&err, 0, 5_000, 30_000), None);
    }

    #[test]
    fn next_retry_budget_ms_stops_instead_of_overrunning_an_exhausted_budget() {
        // Regression guard: the loop used to clamp a fully-consumed budget to 1ms and fire one
        // more subprocess call anyway, silently overrunning the caller's timeout.
        let err = Error::MissingResultField("`herdr agent wait` had no `result` field".to_string());

        assert_eq!(next_retry_budget_ms(&err, 0, 30_000, 30_000), None);
        assert_eq!(next_retry_budget_ms(&err, 0, 40_000, 30_000), None);
    }

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
        assert_eq!(
            find_tab_id_by_label("not json", "herdr-linear-config"),
            None
        );
    }

    #[test]
    fn find_tab_id_by_label_returns_none_when_tabs_array_is_missing() {
        assert_eq!(find_tab_id_by_label(r#"{}"#, "herdr-linear-config"), None);
    }
}
