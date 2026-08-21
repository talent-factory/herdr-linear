//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `implement_one`, shared by both the single- and
//! multi-issue callers) to open a tab, type the launch command into it, wait for the resulting
//! agent to become ready, and inject text. (The `c` keybinding's config-editor hand-off no
//! longer goes through here at all — it runs the editor in-place instead; see
//! `main.rs::run_editor_in_terminal`.) The subprocess-spawning half is deliberately untested at
//! this layer — there's no lower-level abstraction here worth mocking; see
//! docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for why. (This no longer holds
//! for the `o` key's `open::that(url)` call, which TF-652's `main.rs::open_issue_url` now wraps
//! behind an injectable `opener` closure and unit-tests directly — see that function's doc
//! comment.) The response-interpretation half (`interpret_output`) is pure and unit-tested below.
//!
//! ## `agent_wait`'s retry-on-missing-`result` workaround
//!
//! herdr v0.7.3 has a reproducible bug: `herdr agent wait --until idle`'s underlying
//! `events.subscribe` stream closes as soon as the target pane's *agent identity* is first
//! detected (e.g. `previous_agent=None → agent=Some(Claude)`, logged the moment herdr notices
//! which CLI is running in the pane), not when its *status* actually reaches the requested
//! value — confirmed via `herdr-server.log` on every observed agent launch, each followed
//! within ~200ms by `outcome="stream_closed"`, long before the agent could plausibly be idle.
//! (The flag was spelled `--status` when this was first diagnosed; herdr 0.8.0 renamed it to
//! `--until`, TF-624 — the underlying stream-close behavior is unchanged.)
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
/// word [`upgrade_hint`]'s upgrade message.
const MIN_HERDR_VERSION: &str = "0.8.0";

/// Substring patterns in herdr's own CLI-parser rejection messages that indicate an installed
/// `herdr` binary predates the 0.8.0 agent-CLI redesign (TF-624). The plugin unconditionally uses
/// these subcommands/options, so any such rejection means the binary is too old. Matches only
/// observed/expected wordings — not a general "any unknown subcommand/option" detector — so an
/// unrelated typo elsewhere isn't misattributed to a version mismatch.
const TOO_OLD_HERDR_PATTERNS: &[&str] = &[
    "unknown option: --cwd",
    "unknown option: --until",
    "unknown subcommand: run",
    "unknown subcommand: prompt",
    "unknown subcommand: rename",
];

/// If `message` matches one of the known herdr-CLI rejection patterns for features introduced in
/// herdr 0.8.0, returns an upgrade hint to append to it. Without this hint, raw
/// "unknown subcommand/option" errors from a too-old herdr give no indication that the fix is to
/// upgrade herdr rather than anything on the plugin side.
fn upgrade_hint(message: &str) -> Option<String> {
    if TOO_OLD_HERDR_PATTERNS
        .iter()
        .any(|pattern| message.contains(pattern))
    {
        Some(format!(
            "this herdr installation appears to be older than {MIN_HERDR_VERSION}; herdr-linear \
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
/// appended — see [`upgrade_hint`] — when the installed `herdr` is too old for the 0.8.0 CLI
/// surface (TF-604, TF-624). Split out from `run` so this logic — the part that actually decides
/// success vs. failure — is unit-testable without spawning a process.
///
/// herdr >= 0.8.0 writes its JSON error body to **stderr** on failure, leaving stdout empty
/// (verified live: `herdr agent wait <unknown-pane> --until idle` exits 1 and prints
/// `{"error":{"code":"agent_not_found",...}}` to stderr — TF-624). The structured error is
/// therefore looked up in *both* streams; parsing stdout alone would never see it, so the
/// `agent_not_found` mapping below would never fire and `agent_wait` would fail on the first
/// poll instead of retrying until herdr's agent detection catches up.
///
/// A successful exit with empty stdout is treated as success (`Value::Null`). Some herdr
/// subcommands — notably `pane run` — produce no output on success (they only type a command
/// into a pane), so requiring parseable JSON there would falsely report failure.
fn interpret_output(
    command_desc: &str,
    status_success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Value> {
    if let Some(err) = output_error(command_desc, status_success, stdout, stderr) {
        return Err(err);
    }

    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();

    // `herdr pane run` exits 0 and prints nothing on success — don't treat that as failure.
    if parsed.is_none() && stdout.trim().is_empty() {
        return Ok(Value::Null);
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

/// The failure half of [`interpret_output`], split out so [`run_raw`] can apply the exact same
/// failure mapping to subcommands whose *success* output isn't a JSON envelope. Returns `None`
/// when the invocation succeeded (zero exit and no error body in either stream).
fn output_error(
    command_desc: &str,
    status_success: bool,
    stdout: &str,
    stderr: &str,
) -> Option<Error> {
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let stderr_parsed: Option<Value> = serde_json::from_str(stderr.trim()).ok();

    let error_obj = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .or_else(|| stderr_parsed.as_ref().and_then(|v| v.get("error")));

    let error_message = error_obj
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string);

    if !status_success || error_message.is_some() {
        // herdr's agent subcommands return `agent_not_found` when the target pane hasn't been
        // identified as an agent yet. Surface it as a distinct variant so `agent_wait` can poll
        // after `pane_run` rather than failing immediately.
        if let Some(code) = error_obj
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
        {
            if code == "agent_not_found" {
                let target = error_message.clone().unwrap_or_default();
                return Some(Error::AgentNotFound(target));
            }
        }

        let message = error_message.unwrap_or_else(|| {
            let stderr = stderr.trim();
            if stderr.is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.to_string()
            }
        });
        let message = match upgrade_hint(&message) {
            Some(hint) => format!("{message} — {hint}"),
            None => message,
        };
        return Some(Error::Internal(format!(
            "`{command_desc}` failed: {message}"
        )));
    }

    None
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

/// True if `error` is herdr's `agent_not_found` response: the target pane isn't tracked as an
/// agent yet. After `pane_run` types a launch command, herdr needs a short moment to observe the
/// process and identify it as a coding agent, so `agent_wait` polls on this error.
fn is_agent_not_found_response(error: &Error) -> bool {
    matches!(error, Error::AgentNotFound(_))
}

/// Wall-clock ceiling for `herdr` subprocess calls that don't carry their own `--timeout`
/// argument. Reached two ways: routed through [`run`] for most subcommands (`agent_list`,
/// `tab_create`, `agent_prompt`, `agent_read`, `pane_run`, `agent_rename`), and passed directly
/// to [`run_with_timeout`] by [`agent_wait_for_exit`]'s and [`agent_wait_for_start`]'s per-poll
/// `agent get` calls, which need this same bound but don't go through [`run`] — see those
/// functions' docs. Without this, a
/// hung `herdr` daemon blocks the single-threaded TUI's event loop indefinitely — `agent_wait` is
/// the exception, since it computes its own call-specific bound in [`agent_wait`] instead of
/// using this constant.
const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Max attempts and per-attempt backoff for [`spawn_with_etxtbsy_retry`]'s retry loop —
/// `ETXTBSY` always resolves on its own once whatever briefly held the executable open
/// for writing lets go, so a handful of short retries reliably rides out the window
/// without meaningfully affecting real (non-`ETXTBSY`) latency.
const ETXTBSY_MAX_RETRIES: u32 = 5;
const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Whether an abandoned `herdr` CLI subprocess call (its future dropped before completion —
/// timed out, or the enclosing task itself was dropped) should have its child process killed, or
/// left to run to completion detached. Every short-`DEFAULT_CLI_TIMEOUT`-bounded call (`run`,
/// `run_raw`) defaults to [`OnAbandon::LeaveRunning`], preserving the long-standing assumption
/// documented at `main.rs`'s `implement_one` (a `pane_run` client-side timeout doesn't necessarily
/// mean the server-side action didn't happen — see that comment) — flipping that for every call
/// risked silently cutting off an in-flight request we can't prove has already reached herdr.
/// [`agent_wait_for_exit`]'s up-to-24h exit-poll (`close_tab_once_agent_has_exited`, TF-649/
/// TF-668) and [`agent_wait_for_start`]'s own poll (TF-669) are the exception: neither is a
/// mutating call whose in-flight completion we need to protect (both issue read-only `agent get`
/// requests), and nothing is holding a `--timeout`-bounded budget open on either's behalf once
/// the enclosing task is dropped (the plugin quit mid-wait), so leaving an in-flight poll's child
/// running would mean an orphaned `herdr agent get` process surviving the plugin itself, however
/// briefly — see `spawn_with_etxtbsy_retry`'s doc for the mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnAbandon {
    /// Let the child keep running to completion even if this call is abandoned. The default for
    /// every short-timeout call — see [`OnAbandon`]'s own doc for why.
    LeaveRunning,
    /// Kill the child as soon as this call is abandoned. Used by [`agent_wait_for_exit`]'s
    /// exit-poll (TF-649/TF-668) and [`agent_wait_for_start`]'s poll (TF-669) — see
    /// [`OnAbandon`]'s own doc for why.
    KillChild,
}

/// Spawns `herdr_bin args`, retrying up to [`ETXTBSY_MAX_RETRIES`] times with a short
/// delay if the OS reports `ErrorKind::ExecutableFileBusy` ("text file busy" / `ETXTBSY`)
/// — a transient condition (something else has the executable open for writing at the
/// exact instant of `execve`), not a real, persistent failure. Most reliably hit by this
/// crate's own tests, which write and `chmod` a fresh fake `herdr` script then exec it
/// almost immediately (a known kernel/VFS race on some CI filesystems) — but the same
/// condition could in principle hit a real `herdr` binary that's mid-reinstall/update at
/// the exact moment this runs, so the retry lives here rather than only in test scaffolding.
///
/// `on_abandon`: with [`OnAbandon::KillChild`], Tokio sends the child a kill signal as soon as
/// its `Child` handle is dropped (e.g. this future being cancelled by an outer
/// `tokio::time::timeout`, or the whole task being dropped) — without it, neither Tokio's runtime
/// shutdown nor a closed stdout/stderr pipe (nothing is written to it until the call actually
/// succeeds) kills an orphaned child by itself, so it keeps running detached until it exits on
/// its own. See [`OnAbandon`] for which calls use which behavior and why.
async fn spawn_with_etxtbsy_retry(
    herdr_bin: &str,
    args: &[&str],
    on_abandon: OnAbandon,
) -> std::io::Result<std::process::Output> {
    let mut attempt = 0;
    loop {
        match Command::new(herdr_bin)
            .args(args)
            .kill_on_drop(on_abandon == OnAbandon::KillChild)
            .output()
            .await
        {
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

/// Spawn a `herdr` CLI subcommand (via [`spawn_with_etxtbsy_retry`]), bounded by
/// `call_timeout`. Shared by [`run_with_timeout`] (JSON-`result` success) and [`run_raw`] (raw
/// text success).
async fn spawn_output(
    herdr_bin: &str,
    args: &[&str],
    call_timeout: Duration,
    on_abandon: OnAbandon,
) -> Result<std::process::Output> {
    let command_desc = format!("{herdr_bin} {}", args.join(" "));

    tokio::time::timeout(
        call_timeout,
        spawn_with_etxtbsy_retry(herdr_bin, args, on_abandon),
    )
    .await
    .map_err(|_| {
        Error::Internal(format!(
            "`{command_desc}` timed out after {call_timeout:?} waiting for herdr"
        ))
    })?
    .map_err(|e| Error::Internal(format!("Failed to run `{herdr_bin}`: {e}")))
}

/// Run a `herdr` CLI subcommand, bounded by `call_timeout`, returning the parsed `result` field
/// on success. See [`interpret_output`] for the success/failure mapping.
async fn run_with_timeout(
    herdr_bin: &str,
    args: &[&str],
    call_timeout: Duration,
    on_abandon: OnAbandon,
) -> Result<Value> {
    let command_desc = format!("{herdr_bin} {}", args.join(" "));
    let output = spawn_output(herdr_bin, args, call_timeout, on_abandon).await?;

    interpret_output(
        &command_desc,
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// [`run_with_timeout`] bounded by [`DEFAULT_CLI_TIMEOUT`] — used by every `herdr` subcommand
/// except `agent_wait`, which computes its own call-specific budget instead (see [`agent_wait`]),
/// and [`agent_wait_for_exit`]'s/[`agent_wait_for_start`]'s per-poll `agent get` calls, which
/// call [`run_with_timeout`] directly rather than going through this function (see those
/// functions' docs).
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(
        herdr_bin,
        args,
        DEFAULT_CLI_TIMEOUT,
        OnAbandon::LeaveRunning,
    )
    .await
}

/// [`run`]'s sibling for subcommands whose success output is raw text rather than a JSON
/// envelope: herdr >= 0.8.0's `agent read` prints the pane's rendered terminal content straight
/// to stdout (TF-624). The failure mapping is [`output_error`]'s, unchanged (a JSON error body
/// on stderr, a non-zero exit, ...) — only the success shape differs.
async fn run_raw(herdr_bin: &str, args: &[&str]) -> Result<String> {
    let command_desc = format!("{herdr_bin} {}", args.join(" "));
    let output = spawn_output(
        herdr_bin,
        args,
        DEFAULT_CLI_TIMEOUT,
        OnAbandon::LeaveRunning,
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if let Some(err) = output_error(&command_desc, output.status.success(), &stdout, &stderr) {
        return Err(err);
    }
    Ok(stdout)
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
/// already-running tab. `root_pane_id` is the pane every caller then actually works in: it comes
/// up at an interactive shell prompt, and [`pane_run`] types the agent/editor command straight
/// into it, so it *is* the agent's pane — it must never be closed (TF-624; before herdr 0.8.0's
/// CLI redesign the launch call instead split a separate agent pane alongside it, which is why
/// callers used to close this one as redundant). See
/// docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for why creating a
/// pre-labeled tab replaced the older rename-after-launch sequence.
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

/// `herdr tab close <tab_id>` — closes the whole tab, along with every pane inside it (in
/// practice just [`TabCreated::root_pane_id`], see [`tab_create`]'s docs on why that pane is
/// never closed *by itself*). Closing the tab as a unit rather than the pane sidesteps that
/// distinction entirely — there is nothing left in it to keep alive. Used by `main.rs`'s
/// close-on-done watcher (TF-649) to tidy up a per-issue tab once its agent finishes.
pub async fn tab_close(herdr_bin: &str, tab_id: &TabId) -> Result<()> {
    run(herdr_bin, &["tab", "close", tab_id.as_str()])
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

/// `herdr agent rename <pane_id> <name>` — assigns a friendly display name to a pane herdr has
/// already recognized as hosting a coding agent (requires the target to already be
/// auto-detected; fails with `agent_not_found` otherwise — verified live against herdr 0.8.0).
/// Used by `main.rs`'s `implement_one` purely cosmetically, to preserve the per-issue names
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

/// Pause between `agent_wait` polls when herdr reports `agent_not_found`. `pane_run` only types
/// a command into the pane; herdr needs time to observe the resulting process and classify it as
/// a coding agent before `agent wait --until idle` can succeed. A short fixed sleep prevents
/// spamming herdr while still converging well within the caller's timeout budget. `pub` (not
/// module-private) since TF-669's `main.rs::implement_one` — a separate binary crate from this
/// module's `herdr_linear` library crate — reuses it as [`agent_wait_for_start`]'s poll interval
/// too, rather than duplicating the same "how often is it reasonable to ask herdr" magic number.
pub const AGENT_NOT_FOUND_POLL_INTERVAL: Duration = Duration::from_millis(500);

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

/// `herdr agent wait <pane_id> --until <status> --timeout <timeout_ms>` (herdr 0.8.0 renamed
/// this subcommand's `--status` flag to `--until`, TF-624). Retries, within the caller's
/// original `timeout_ms` budget, when herdr responds with the missing-`result` bug documented at
/// the top of this module — any other error (a real timeout, no such pane, ...) returns
/// immediately. `on_abandon` controls what happens to the underlying `herdr` subprocess if this
/// call itself is abandoned before completing (e.g. its enclosing task dropped) — see
/// [`OnAbandon`] for which value to pass and why.
pub async fn agent_wait(
    herdr_bin: &str,
    pane_id: &PaneId,
    status: &str,
    timeout_ms: u64,
    on_abandon: OnAbandon,
) -> Result<()> {
    let start = std::time::Instant::now();
    let mut attempt = 0;

    loop {
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let remaining_ms = timeout_ms.saturating_sub(elapsed_ms);
        if remaining_ms == 0 {
            return Err(Error::Internal(format!(
                "`herdr agent wait` timed out after {timeout_ms}ms waiting for agent to become {status}"
            )));
        }

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
            on_abandon,
        )
        .await;

        let err = match result {
            Ok(_) => return Ok(()),
            Err(err) => err,
        };

        // `agent_not_found` means herdr hasn't classified the pane as an agent yet. Keep polling
        // until the caller's timeout is exhausted rather than giving up immediately.
        if is_agent_not_found_response(&err) {
            tokio::time::sleep(AGENT_NOT_FOUND_POLL_INTERVAL).await;
            continue;
        }

        match next_retry_budget_ms(&err, attempt, elapsed_ms, timeout_ms) {
            Some(_) => {
                attempt += 1;
            }
            None => return Err(err),
        }
    }
}

/// Consecutive `agent_not_found` polls [`agent_wait_for_exit`] requires, in a row, before
/// concluding the pane's agent has genuinely exited, rather than trusting a single observation —
/// see that function's doc for why.
const AGENT_EXIT_CONFIRM_POLLS: u32 = 3;

/// Consecutive non-`agent_not_found` poll errors [`agent_wait_for_exit`] tolerates (e.g. a
/// [`DEFAULT_CLI_TIMEOUT`] expiry under load, a momentary herdr daemon hiccup) before giving up
/// and returning the error to its caller — see that function's doc for why.
const AGENT_EXIT_POLL_ERROR_TOLERANCE: u32 = 3;

/// TF-668: waits until herdr no longer recognizes `pane_id` as hosting a live agent — i.e. a
/// `herdr agent get <pane_id>` call starts failing with the `agent_not_found` error code
/// `AGENT_EXIT_CONFIRM_POLLS` times in a row — which typically means the interactive
/// coding-agent process actually terminated (the user or agent typed `/exit`). herdr's own docs
/// also name two narrower cases producing that same error code — the pane being `release`d, or
/// replaced by another process — that this module hasn't independently verified live against a
/// real herdr instance; requiring a run of consecutive polls rather than trusting a single one is
/// exactly what keeps either of those, or a transient herdr-side identity-tracking blip, from
/// being mistaken for the pane's agent actually being gone.
///
/// Deliberately does *not* poll [`agent_wait`] for herdr's `"done"` status: herdr's own skill doc
/// defines `done` as "the same underlying idle state [as `idle`] after unseen background work
/// finishes" — a tab-focus-tracking heuristic with no relationship to whether the agent's actual
/// work (opening a PR, getting it reviewed, fixing findings, ...) is complete. It fires the
/// moment the agent goes idle after just the initial implement prompt, long before those manual
/// steps happen — see `main.rs`'s `close_tab_once_agent_has_exited` for the full TF-668 writeup.
/// Waiting for the pane's agent to truly disappear ties the wait to something real instead: the
/// session in that pane is over.
///
/// Polls every `poll_interval` while the agent is still present. Any error other than
/// `agent_not_found` is tolerated up to `AGENT_EXIT_POLL_ERROR_TOLERANCE` times in a row before
/// propagating: unlike [`agent_wait`]'s single long-lived subprocess call, this function can issue
/// thousands of individual polls over its up-to-24h budget, so one transient failure shouldn't
/// abort the whole wait the way it reasonably can for a single call. Both consecutive-count
/// thresholds reset to zero on every poll that doesn't match them, so only a genuine run of either
/// kind ever fires — isolated blips scattered across the wait never accumulate toward either
/// threshold. `on_abandon` is forwarded to each individual poll call — see [`OnAbandon`]'s doc —
/// though it matters far less here than it did for the `agent_wait(..., "done", ...)` call this
/// replaces: no single poll here ever blocks longer than `DEFAULT_CLI_TIMEOUT`, rather than one
/// subprocess held open for the caller's entire (up to 24h) budget.
pub async fn agent_wait_for_exit(
    herdr_bin: &str,
    pane_id: &PaneId,
    timeout_ms: u64,
    poll_interval: Duration,
    on_abandon: OnAbandon,
) -> Result<()> {
    let start = std::time::Instant::now();
    let mut consecutive_not_found: u32 = 0;
    let mut consecutive_errors: u32 = 0;

    loop {
        match run_with_timeout(
            herdr_bin,
            &["agent", "get", pane_id.as_str()],
            DEFAULT_CLI_TIMEOUT,
            on_abandon,
        )
        .await
        {
            Err(err) if is_agent_not_found_response(&err) => {
                consecutive_errors = 0;
                consecutive_not_found += 1;
                if consecutive_not_found >= AGENT_EXIT_CONFIRM_POLLS {
                    return Ok(());
                }
            }
            Err(err) => {
                consecutive_not_found = 0;
                consecutive_errors += 1;
                if consecutive_errors >= AGENT_EXIT_POLL_ERROR_TOLERANCE {
                    return Err(err);
                }
            }
            Ok(_) => {
                consecutive_not_found = 0;
                consecutive_errors = 0;
            }
        }

        if start.elapsed().as_millis() as u64 >= timeout_ms {
            return Err(Error::Internal(format!(
                "agent in {pane_id:?} never exited within {timeout_ms}ms"
            )));
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Consecutive `herdr agent get` polls reporting the *same* agent identity
/// [`agent_wait_for_start`] requires before concluding a real, stable coding agent has started —
/// see that function's doc for why a single matching poll isn't enough.
const AGENT_START_CONFIRM_POLLS: u32 = 3;

/// Consecutive non-`agent_not_found` poll errors [`agent_wait_for_start`] tolerates before giving
/// up and returning the error to its caller — mirrors [`AGENT_EXIT_POLL_ERROR_TOLERANCE`].
const AGENT_START_POLL_ERROR_TOLERANCE: u32 = 3;

/// Extracts the `agent` identity string from a `herdr agent get`/`agent list` entry's `result`
/// value (e.g. `{"agent": {"agent": "claude", ...}}` — confirmed live against a real herdr 0.8.0
/// instance). Returns `None` for a missing/blank field rather than treating it as a distinct
/// identity, so a malformed response can't be mistaken for a real (if odd) agent name.
fn agent_identity(result: &serde_json::Value) -> Option<String> {
    let name = result.get("agent")?.get("agent")?.as_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// TF-669: waits until herdr recognizes `pane_id` as hosting a real, *stable* coding agent — i.e.
/// `herdr agent get <pane_id>` reports the same non-blank `agent` identity on
/// `AGENT_START_CONFIRM_POLLS` consecutive polls — before the caller trusts the pane enough to
/// start sending it input.
///
/// Exists because neither `agent_wait(..., "idle", ...)`'s status nor a purely text-based
/// "did the prompt land" check (`plugin::implement`'s `prompt_landed`, driven by `main.rs`'s
/// `wait_for_prompt_stable`) can tell
/// "a real agent's live input box" apart from "a wrapper's own plain, pre-agent bootstrap output
/// that just happens to look quiet or contain matching text for a while". Live-reproduced
/// (TF-669) against a multi-stage `agent_command` alias (`hr` = `headroom wrap claude
/// --memory --code-graph`): its own bootstrap (memory sync, a local proxy check, rtk hook setup)
/// can run for several seconds of plain shell output *before* `claude` is even exec'd, during
/// which `herdr agent get` reliably reports `agent_not_found` — confirmed by directly querying a
/// live pane caught mid-race. Requiring the *same* identity on several consecutive polls, not
/// just one, additionally guards against herdr transiently misidentifying one of the wrapper's
/// own intermediate helper processes as "the agent" before the real target takes over: a single
/// matching poll resets and restarts the count on the next poll's *different* identity, rather
/// than declaring success on a value that's about to change.
///
/// Poll/timeout/tolerance semantics otherwise mirror [`agent_wait_for_exit`] with the transition
/// inverted: an `agent_not_found` response is the *expected* steady state before the agent has
/// started (resets the confirmation streak, same as that function's `Ok` branch resetting its
/// exit-confirmation streak), while a matching identity is the awaited signal. `on_abandon` is
/// forwarded to each individual poll call — see [`OnAbandon`]'s doc.
///
/// `on_poll` fires once per completed poll (1-indexed), regardless of that poll's outcome — a
/// non-blocking progress hook so a caller with a UI to redraw (`implement_one`, via `main.rs`'s
/// `agent_start_poll_status`) isn't left with no signal for this wait's own duration the way
/// `agent_wait`'s "idle" wait once was before `prompt_attempt_status` (TF-650) fixed that for the
/// *later* prompt-send phase. Pass a no-op (`|_| {}`) if the caller has nothing to redraw.
pub async fn agent_wait_for_start(
    herdr_bin: &str,
    pane_id: &PaneId,
    timeout_ms: u64,
    poll_interval: Duration,
    on_abandon: OnAbandon,
    mut on_poll: impl FnMut(u32) + Send,
) -> Result<()> {
    let start = std::time::Instant::now();
    let mut consecutive_confirmed: u32 = 0;
    let mut consecutive_errors: u32 = 0;
    let mut last_identity: Option<String> = None;
    let mut poll_count: u32 = 0;
    // TF-669: human-readable summary of the *last* poll's outcome, folded into the timeout error
    // below so a stuck-pane report says *why* the wait never completed (still agent_not_found?
    // an identity that never stabilized? a response herdr answered but that carried no
    // identity?) instead of just restating the timeout it hit either way.
    let mut last_state: String;

    loop {
        match run_with_timeout(
            herdr_bin,
            &["agent", "get", pane_id.as_str()],
            DEFAULT_CLI_TIMEOUT,
            on_abandon,
        )
        .await
        {
            Ok(value) => match agent_identity(&value) {
                Some(identity) => {
                    consecutive_errors = 0;
                    if last_identity.as_deref() == Some(identity.as_str()) {
                        consecutive_confirmed += 1;
                    } else {
                        last_identity = Some(identity.clone());
                        consecutive_confirmed = 1;
                    }
                    if consecutive_confirmed >= AGENT_START_CONFIRM_POLLS {
                        return Ok(());
                    }
                    last_state = format!(
                        "identity {identity:?} seen on {consecutive_confirmed}/\
                         {AGENT_START_CONFIRM_POLLS} consecutive polls so far"
                    );
                }
                None => {
                    consecutive_errors = 0;
                    consecutive_confirmed = 0;
                    last_identity = None;
                    // A successful response that carries no identity is a distinct case from
                    // herdr explicitly reporting `agent_not_found` (see `agent_identity`'s doc)
                    // — it's also the shape a herdr protocol change would take if the `agent`
                    // field were ever renamed or retyped. Nothing else in this loop retains the
                    // raw response, so without this it degrades into indistinguishable
                    // "not started yet" polling with no trail for a future investigation.
                    tracing::debug!(
                        response = %value,
                        "agent_wait_for_start: `herdr agent get {}` succeeded but the response \
                         carried no parseable agent identity",
                        pane_id.as_str()
                    );
                    last_state =
                        "agent get succeeded but returned no parseable identity (see debug log \
                         for the raw response)"
                            .to_string();
                }
            },
            Err(err) if is_agent_not_found_response(&err) => {
                consecutive_errors = 0;
                consecutive_confirmed = 0;
                last_identity = None;
                last_state = "still agent_not_found".to_string();
            }
            Err(err) => {
                consecutive_confirmed = 0;
                last_identity = None;
                consecutive_errors += 1;
                if consecutive_errors >= AGENT_START_POLL_ERROR_TOLERANCE {
                    return Err(err);
                }
                last_state = format!(
                    "tolerating error {consecutive_errors}/{AGENT_START_POLL_ERROR_TOLERANCE} \
                     before giving up: {err}"
                );
            }
        }

        poll_count += 1;
        on_poll(poll_count);

        if start.elapsed().as_millis() as u64 >= timeout_ms {
            return Err(Error::Internal(format!(
                "agent in {pane_id:?} never started within {timeout_ms}ms ({last_state})"
            )));
        }

        tokio::time::sleep(poll_interval).await;
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

/// `herdr agent read <pane_id> --source <source> --lines <lines>` — the pane's rendered
/// terminal text. herdr >= 0.8.0 prints it straight to stdout as raw text — the JSON envelope
/// other subcommands use does not apply here (TF-624) — while failures (an unknown or
/// not-yet-detected pane) still arrive as a JSON error body on stderr; the private `run_raw`
/// helper maps both.
/// Used by `main.rs`'s `send_prompt_until_visible` to confirm an [`agent_prompt`]
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
    run_raw(
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
    .await
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
    fn interpret_output_treats_empty_stdout_with_success_exit_as_success() {
        // `herdr pane run` exits 0 and prints nothing on success. Before this special case it
        // was reported as "unparseable output", which made every issue launch look like it had
        // failed even though the agent command was typed into the pane correctly.
        let result = interpret_output("herdr pane run wY:p7X hr", true, "", "");

        assert_eq!(result.unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn interpret_output_surfaces_agent_not_found_as_distinct_variant() {
        // herdr >= 0.8.0 prints its JSON error body to stderr, not stdout (verified live,
        // TF-624) — the mapping must fire regardless of which stream carries the body.
        let body = r#"{"error":{"code":"agent_not_found","message":"agent target wY:p7Z not found"},"id":"cli:agent:wait"}"#;
        for (stdout, stderr) in [(body, ""), ("", body)] {
            let err = interpret_output(
                "herdr agent wait wY:p7Z --until idle --timeout 30000",
                false,
                stdout,
                stderr,
            )
            .unwrap_err();

            assert!(
                matches!(err, Error::AgentNotFound(_)),
                "expected AgentNotFound (stdout={stdout:?}, stderr={stderr:?}), got: {err:?}"
            );
        }
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
    fn interpret_output_hints_at_upgrading_herdr_for_other_08_only_features() {
        // TF-624 review: a herdr between the old floor and 0.8.0 accepts `--cwd` on `tab create`
        // but then fails on one of the redesigned agent-CLI commands (`pane run`, `agent prompt`,
        // `agent wait --until`, `agent rename`) with no version hint.
        for (command, stderr) in [
            ("herdr pane run wY:p1 nvim", "unknown subcommand: run\n"),
            (
                "herdr agent prompt wY:p1 hi",
                "unknown subcommand: prompt\n",
            ),
            (
                "herdr agent wait --until idle wY:p1",
                "unknown option: --until\n",
            ),
            (
                "herdr agent rename wY:p1 Linear",
                "unknown subcommand: rename\n",
            ),
        ] {
            let result = interpret_output(command, false, "", stderr);
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains(MIN_HERDR_VERSION),
                "expected upgrade hint for {command}, got: {err}"
            );
        }
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
    /// `agent_wait`, ...) can be exercised end-to-end without a real `herdr` daemon. Each such
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
    async fn tab_close_builds_the_expected_cli_invocation() {
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

        tab_close(script.to_str().unwrap(), &TabId("t2".to_string()))
            .await
            .expect("tab_close should succeed");

        let captured = std::fs::read_to_string(&args_file).unwrap();
        let args: Vec<&str> = captured.lines().collect();
        assert_eq!(args, vec!["tab", "close", "t2"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tab_close_propagates_a_herdr_error() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"message":"no such tab"}}'
exit 1
"#,
        );

        let err = tab_close(script.to_str().unwrap(), &TabId("t2".to_string()))
            .await
            .expect_err("tab_close should propagate the herdr error");

        assert!(
            err.to_string().contains("no such tab"),
            "unexpected message: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_polls_through_agent_not_found_until_success() {
        // `agent_wait` should not fail on the first `agent_not_found`; after `pane_run` types a
        // command, herdr needs time to detect the agent. A fake script returns `agent_not_found`
        // twice, then success.
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -le 2 ]; then
  # Real herdr >= 0.8.0 prints its JSON error body to stderr (TF-624).
  echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}},"id":"cli:agent:wait"}}' >&2
  exit 1
fi
echo '{{"result":{{}},"id":"cli:agent:wait"}}'
"#,
            counter_file.display()
        ));

        agent_wait(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            "idle",
            5_000,
            OnAbandon::LeaveRunning,
        )
        .await
        .expect("agent_wait should poll through agent_not_found and succeed");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "3",
            "expected three calls before success"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_times_out_when_agent_not_found_persists() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
# Real herdr >= 0.8.0 prints its JSON error body to stderr (TF-624).
echo '{"error":{"code":"agent_not_found","message":"agent target wY:p7Z not found"},"id":"cli:agent:wait"}' >&2
exit 1
"#,
        );

        let err = agent_wait(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            "idle",
            100,
            OnAbandon::LeaveRunning,
        )
        .await
        .expect_err("agent_wait should time out when agent_not_found persists");

        assert!(
            err.to_string().contains("timed out"),
            "expected timeout error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_returns_ok_once_agent_not_found_is_confirmed_consecutively() {
        // `agent get` succeeds (the agent is still there) twice, then reports
        // `agent_not_found` on `AGENT_EXIT_CONFIRM_POLLS` (3) consecutive polls — simulating
        // the pane's agent process actually terminating (e.g. `/exit`) partway through the
        // wait.
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -le 2 ]; then
  echo '{{"result":{{"agent_status":"idle"}}}}'
  exit 0
fi
echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}}}' >&2
exit 1
"#,
            counter_file.display()
        ));

        agent_wait_for_exit(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
        )
        .await
        .expect("agent_wait_for_exit should succeed once agent_not_found is confirmed");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "5",
            "expected 2 idle polls plus AGENT_EXIT_CONFIRM_POLLS (3) consecutive not-found polls"
        );
    }

    /// TF-668 regression test: a *lone* `agent_not_found` poll, surrounded by the agent still
    /// being reported present, must never be mistaken for the agent having genuinely exited —
    /// this is exactly the false-positive a single-sample check would fall for (a herdr-side
    /// identity-tracking blip, a `release`/reclassify race, ...), and is why
    /// `AGENT_EXIT_CONFIRM_POLLS` requires a *consecutive* run instead of trusting one
    /// observation. The agent here never actually goes away, so the wait must time out — it
    /// must never return `Ok`.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_does_not_return_ok_on_a_single_transient_agent_not_found() {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -eq 2 ]; then
  echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}}}' >&2
  exit 1
fi
echo '{{"result":{{"agent_status":"idle"}}}}'
exit 0
"#,
            counter_file.display()
        ));

        let err = agent_wait_for_exit(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            60,
            Duration::from_millis(10),
            OnAbandon::LeaveRunning,
        )
        .await
        .expect_err(
            "a single transient agent_not_found must not be mistaken for the agent having exited",
        );

        assert!(
            err.to_string().contains("never exited"),
            "expected a never-exited timeout error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_propagates_a_non_agent_not_found_error_after_the_tolerance_is_exhausted(
    ) {
        let capture_dir = tempfile::tempdir().unwrap();
        let counter_file = capture_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
echo $((count + 1)) > "$count_file"
echo '{{"error":{{"message":"no such pane"}}}}' >&2
exit 1
"#,
            counter_file.display()
        ));

        let err = agent_wait_for_exit(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
        )
        .await
        .expect_err("agent_wait_for_exit should propagate a persistent non-agent_not_found error");

        assert!(
            err.to_string().contains("no such pane"),
            "unexpected message: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&counter_file).unwrap().trim(),
            "3",
            "expected exactly AGENT_EXIT_POLL_ERROR_TOLERANCE (3) attempts before giving up"
        );
    }

    /// TF-668 regression test for the flip side of the tolerance above: an *isolated* transient
    /// error, surrounded by successful polls, must not push the wait any closer to giving up —
    /// only a genuine *run* of `AGENT_EXIT_POLL_ERROR_TOLERANCE` consecutive failures should. The
    /// agent then genuinely exits, so the wait must still succeed rather than erroring out on an
    /// error count that never actually reached the threshold in a row.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_tolerates_an_isolated_transient_error_and_keeps_polling() {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -eq 1 ]; then
  echo '{{"error":{{"message":"herdr daemon hiccup"}}}}' >&2
  exit 1
fi
if [ "$next" -le 4 ]; then
  echo '{{"result":{{"agent_status":"idle"}}}}'
  exit 0
fi
echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}}}' >&2
exit 1
"#,
            counter_file.display()
        ));

        agent_wait_for_exit(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
        )
        .await
        .expect("an isolated transient error must not stop the wait from eventually succeeding");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "7",
            "expected 1 transient error + 3 idle polls + AGENT_EXIT_CONFIRM_POLLS (3) not-found \
             polls"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_times_out_when_the_agent_never_exits() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"result":{"agent_status":"working"}}'
exit 0
"#,
        );

        let err = agent_wait_for_exit(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            30,
            Duration::from_millis(10),
            OnAbandon::LeaveRunning,
        )
        .await
        .expect_err("agent_wait_for_exit should time out while the agent stays present");

        assert!(
            err.to_string().contains("never exited"),
            "expected a never-exited timeout error, got: {err}"
        );
    }

    /// TF-669: the counterpart to `agent_wait_for_exit`'s consecutive-poll confirmation, but for
    /// the opposite transition (an agent appearing rather than disappearing). `agent get` reports
    /// `agent_not_found` twice (simulating `hr`'s own pre-`claude` bootstrap, which herdr doesn't
    /// yet recognize as hosting any agent), then the same `"claude"` identity on
    /// `AGENT_START_CONFIRM_POLLS` (3) consecutive polls.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_returns_ok_once_the_same_agent_identity_is_confirmed_consecutively(
    ) {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -le 2 ]; then
  echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}}}' >&2
  exit 1
fi
echo '{{"result":{{"agent":{{"agent":"claude"}}}}}}'
exit 0
"#,
            counter_file.display()
        ));

        agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect("agent_wait_for_start should succeed once the same identity is confirmed");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "5",
            "expected 2 agent_not_found polls plus AGENT_START_CONFIRM_POLLS (3) matching polls"
        );
    }

    /// TF-669 regression test: the failure mode this ticket exists for — a multi-stage wrapper
    /// (`hr` = `headroom wrap claude ...`) can cause herdr to transiently misidentify one of its
    /// own intermediate helper processes as "the agent" before the real target agent takes over.
    /// A single matching poll must not be enough — the identity has to stabilize across
    /// `AGENT_START_CONFIRM_POLLS` polls in a row, or a stray/incorrect early identification would
    /// let the caller start sending the implement prompt long before `claude` is actually there.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_resets_the_confirmation_count_when_the_identity_changes() {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -le 2 ]; then
  echo '{{"result":{{"agent":{{"agent":"headroom-rtk-installer"}}}}}}'
  exit 0
fi
echo '{{"result":{{"agent":{{"agent":"claude"}}}}}}'
exit 0
"#,
            counter_file.display()
        ));

        agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect("agent_wait_for_start should succeed once the new identity itself stabilizes");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "5",
            "expected 2 polls of the stray identity (reset by the switch) plus \
             AGENT_START_CONFIRM_POLLS (3) polls of the real one"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_times_out_when_no_agent_is_ever_recognized() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}' >&2
exit 1
"#,
        );

        let err = agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            30,
            Duration::from_millis(10),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect_err("agent_wait_for_start should time out while no agent is ever recognized");

        assert!(
            err.to_string().contains("never started"),
            "expected a never-started timeout error, got: {err}"
        );
    }

    /// The flip side of the tolerance in `agent_wait_for_exit_tolerates_an_isolated_transient_error_and_keeps_polling`:
    /// an isolated non-`agent_not_found` error (a `DEFAULT_CLI_TIMEOUT` blip, a momentary herdr
    /// hiccup) costs the identity-confirmation streak accumulated so far (mirroring
    /// `agent_wait_for_exit`'s `Err(other)` branch resetting its own streak) but must not abort
    /// the wait outright — only a genuine run of such errors ([`AGENT_START_POLL_ERROR_TOLERANCE`])
    /// should.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_tolerates_an_isolated_non_agent_not_found_error() {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -eq 2 ]; then
  echo '{{"error":{{"message":"herdr daemon hiccup"}}}}' >&2
  exit 1
fi
echo '{{"result":{{"agent":{{"agent":"claude"}}}}}}'
exit 0
"#,
            counter_file.display()
        ));

        agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect("an isolated transient error must not stop the wait from eventually succeeding");
    }

    /// TF-669 regression test: the counterpart to
    /// `agent_wait_for_exit_propagates_a_non_agent_not_found_error_after_the_tolerance_is_exhausted`
    /// — a *persistent* run of non-`agent_not_found` errors (not just one, as the isolated-error
    /// test above covers) must be propagated, and after exactly [`AGENT_START_POLL_ERROR_TOLERANCE`]
    /// (3) attempts, not fewer (which would abort too eagerly on a transient blip) or more (which
    /// would leave the caller waiting on a herdr that's genuinely stopped responding).
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_propagates_a_non_agent_not_found_error_after_the_tolerance_is_exhausted(
    ) {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
echo $((count + 1)) > "$count_file"
echo '{{"error":{{"message":"no such pane"}}}}' >&2
exit 1
"#,
            counter_file.display()
        ));

        let err = agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect_err("agent_wait_for_start should propagate a persistent non-agent_not_found error");

        assert!(
            err.to_string().contains("no such pane"),
            "unexpected message: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&counter_file).unwrap().trim(),
            "3",
            "expected exactly AGENT_START_POLL_ERROR_TOLERANCE (3) attempts before giving up"
        );
    }

    /// TF-669 regression test: a genuine `agent_not_found` interrupting an in-progress
    /// confirmation streak must force a full recount, not just resume where it left off — the
    /// pane really did stop reporting an agent, however briefly, so the previously-accumulated
    /// polls can't be trusted to still describe the current state. Mirrors
    /// `agent_wait_for_start_resets_the_confirmation_count_when_the_identity_changes`, but for an
    /// intervening *disappearance* rather than a *different* identity.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_resets_the_confirmation_count_on_an_intervening_agent_not_found()
    {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -eq 2 ]; then
  echo '{{"error":{{"code":"agent_not_found","message":"agent target wY:p7Z not found"}}}}' >&2
  exit 1
fi
echo '{{"result":{{"agent":{{"agent":"claude"}}}}}}'
exit 0
"#,
            counter_file.display()
        ));

        agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect("agent_wait_for_start should still succeed once the identity re-stabilizes");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "5",
            "expected 1 confirming poll, 1 intervening agent_not_found (resetting the streak), \
             then AGENT_START_CONFIRM_POLLS (3) fresh confirming polls"
        );
    }

    /// TF-669 regression test: an `Ok` response that parses but carries no identity (the
    /// `agent_identity` `None` case — a missing/blank `agent.agent` field) must reset the
    /// confirmation streak exactly like an explicit `agent_not_found` error does, not be treated
    /// as a match or silently ignored.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_start_resets_the_confirmation_count_on_a_blank_identity_response() {
        let counter_dir = tempfile::tempdir().unwrap();
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap();
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
count_file="{}"
count=$(cat "$count_file")
next=$((count + 1))
echo "$next" > "$count_file"
if [ "$next" -eq 2 ]; then
  echo '{{"result":{{"agent":{{"agent":""}}}}}}'
  exit 0
fi
echo '{{"result":{{"agent":{{"agent":"claude"}}}}}}'
exit 0
"#,
            counter_file.display()
        ));

        agent_wait_for_start(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            5_000,
            Duration::from_millis(5),
            OnAbandon::LeaveRunning,
            |_| {},
        )
        .await
        .expect("agent_wait_for_start should still succeed once a real identity stabilizes");

        let final_count = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(
            final_count.trim(),
            "5",
            "expected 1 confirming poll, 1 blank-identity response (resetting the streak), then \
             AGENT_START_CONFIRM_POLLS (3) fresh confirming polls"
        );
    }

    /// Polls for `path` to exist, up to `timeout`, checking every 50ms. Used by the
    /// `OnAbandon`/kill-on-drop tests below instead of a single fixed sleep-then-check: under a
    /// full, parallel `cargo test` run, OS scheduling contention can easily push either the fake
    /// script's own 1s `sleep` or this test's wakeup past a tight fixed margin, so a generous
    /// polling ceiling is what actually makes these tests reliable rather than flaky.
    async fn marker_appears_within(path: &std::path::Path, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            if path.exists() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// TF-649 follow-up: verifies [`OnAbandon::KillChild`] actually kills the underlying `herdr`
    /// subprocess when the `agent_wait` call itself is abandoned (here, by racing it against an
    /// outer `tokio::time::timeout` shorter than the fake script's own sleep — the same drop this
    /// codebase would see if the plugin quit while `close_tab_once_agent_has_exited`'s exit-poll
    /// was still in flight). The fake script only writes its completion marker *after* sleeping;
    /// if the child is killed mid-sleep, that marker must never appear, even after polling well
    /// past the sleep's own duration.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_with_kill_on_drop_kills_its_child_when_abandoned() {
        let capture_dir = tempfile::tempdir().unwrap();
        let completed_marker = capture_dir.path().join("completed");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
sleep 1
touch "{}"
echo '{{"result":{{}}}}'
exit 0
"#,
            completed_marker.display()
        ));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            agent_wait(
                script.to_str().unwrap(),
                &PaneId("wY:p7Z".to_string()),
                "done",
                10_000,
                OnAbandon::KillChild,
            ),
        )
        .await;
        assert!(
            outcome.is_err(),
            "expected the outer timeout to abandon agent_wait before the fake script's 1s sleep \
             completed"
        );

        // Proving a negative means waiting out the full ceiling — generous on purpose (see
        // `marker_appears_within`'s doc) so this doesn't flake under parallel test-suite load.
        assert!(
            !marker_appears_within(&completed_marker, Duration::from_secs(5)).await,
            "OnAbandon::KillChild should have killed the herdr subprocess before its 1s sleep \
             finished, but the completion marker appeared — it ran to completion instead"
        );
    }

    /// TF-649 follow-up: the [`OnAbandon::KillChild`] test's negative control — confirms
    /// [`OnAbandon::LeaveRunning`] (every pre-existing `agent_wait` call site) is unaffected by
    /// this parameter's introduction: an abandoned call's child keeps running detached and
    /// completes on its own, exactly as it did before `OnAbandon` existed.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_with_leave_running_lets_its_child_finish_when_abandoned() {
        let capture_dir = tempfile::tempdir().unwrap();
        let completed_marker = capture_dir.path().join("completed");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
sleep 1
touch "{}"
echo '{{"result":{{}}}}'
exit 0
"#,
            completed_marker.display()
        ));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            agent_wait(
                script.to_str().unwrap(),
                &PaneId("wY:p7Z".to_string()),
                "idle",
                10_000,
                OnAbandon::LeaveRunning,
            ),
        )
        .await;
        assert!(
            outcome.is_err(),
            "expected the outer timeout to abandon agent_wait before the fake script's 1s sleep \
             completed"
        );

        assert!(
            marker_appears_within(&completed_marker, Duration::from_secs(5)).await,
            "OnAbandon::LeaveRunning should leave the herdr subprocess running detached — \
             expected it to finish its 1s sleep on its own and write the completion marker"
        );
    }

    /// TF-668 follow-up: [`agent_wait_with_kill_on_drop_kills_its_child_when_abandoned`] above
    /// proves the shared `spawn_with_etxtbsy_retry`/[`run_with_timeout`] primitive honors
    /// [`OnAbandon::KillChild`], but only via [`agent_wait`] — this pins down that
    /// [`agent_wait_for_exit`]'s own call site forwards it correctly too, since that's the exact
    /// drop this codebase sees if the plugin quits while `close_tab_once_agent_has_exited`'s
    /// exit-poll is still in flight.
    #[cfg(unix)]
    #[tokio::test]
    async fn agent_wait_for_exit_with_kill_on_drop_kills_its_child_when_abandoned() {
        let capture_dir = tempfile::tempdir().unwrap();
        let completed_marker = capture_dir.path().join("completed");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
sleep 1
touch "{}"
echo '{{"result":{{"agent_status":"working"}}}}'
exit 0
"#,
            completed_marker.display()
        ));

        let outcome = tokio::time::timeout(
            Duration::from_millis(150),
            agent_wait_for_exit(
                script.to_str().unwrap(),
                &PaneId("wY:p7Z".to_string()),
                10_000,
                Duration::from_millis(10),
                OnAbandon::KillChild,
            ),
        )
        .await;
        assert!(
            outcome.is_err(),
            "expected the outer timeout to abandon agent_wait_for_exit before the fake script's \
             1s sleep completed"
        );

        assert!(
            !marker_appears_within(&completed_marker, Duration::from_secs(5)).await,
            "OnAbandon::KillChild should have killed the herdr subprocess before its 1s sleep \
             finished, but the completion marker appeared — it ran to completion instead"
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

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_read_returns_the_raw_terminal_text() {
        // herdr >= 0.8.0 prints the pane's rendered content as plain text on stdout — no JSON
        // envelope (TF-624).
        let (_dir, script) = write_fake_herdr_script(
            r#"
printf '> Implement Linear Issue TF-579 using a new git worktree\n'
exit 0
"#,
        );

        let text = agent_read(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            "visible",
            60,
        )
        .await
        .expect("agent_read should return the raw terminal text");

        assert!(
            text.contains("Implement Linear Issue TF-579"),
            "unexpected text: {text:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_read_maps_a_stderr_error_body_to_agent_not_found() {
        // Real herdr >= 0.8.0 reports a not-yet-detected pane as a JSON error body on stderr
        // (TF-624) — `agent_read` must surface that through the same mapping as every other
        // subcommand, not fail with "unparseable output" on the empty stdout.
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"code":"agent_not_found","message":"agent target wY:p7Z not found"},"id":"cli:agent:read"}' >&2
exit 1
"#,
        );

        let err = agent_read(
            script.to_str().unwrap(),
            &PaneId("wY:p7Z".to_string()),
            "visible",
            60,
        )
        .await
        .expect_err("agent_read should surface the stderr error body");

        assert!(
            matches!(err, Error::AgentNotFound(_)),
            "expected AgentNotFound, got: {err:?}"
        );
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

    // TF-669: `agent_identity` is [`agent_wait_for_start`]'s pure extraction step — no
    // subprocess, no timing — so it's unit-tested directly rather than only indirectly through
    // the subprocess-scripted `agent_wait_for_start` tests above, mirroring the same rationale
    // `next_retry_budget_ms` was pulled out and tested for.

    #[test]
    fn agent_identity_extracts_the_nested_agent_field() {
        let value = serde_json::json!({"agent": {"agent": "claude"}});
        assert_eq!(agent_identity(&value), Some("claude".to_string()));
    }

    #[test]
    fn agent_identity_trims_surrounding_whitespace() {
        let value = serde_json::json!({"agent": {"agent": "  claude  "}});
        assert_eq!(agent_identity(&value), Some("claude".to_string()));
    }

    #[test]
    fn agent_identity_is_none_for_a_blank_or_whitespace_only_name() {
        assert_eq!(
            agent_identity(&serde_json::json!({"agent": {"agent": ""}})),
            None
        );
        assert_eq!(
            agent_identity(&serde_json::json!({"agent": {"agent": "   "}})),
            None
        );
    }

    #[test]
    fn agent_identity_is_none_when_the_inner_agent_field_is_missing() {
        assert_eq!(agent_identity(&serde_json::json!({"agent": {}})), None);
    }

    #[test]
    fn agent_identity_is_none_when_the_outer_agent_field_is_missing() {
        assert_eq!(agent_identity(&serde_json::json!({})), None);
    }

    #[test]
    fn agent_identity_is_none_for_a_non_string_agent_field() {
        // A malformed/protocol-changed response (wrong JSON type) must not be mistaken for a
        // real — if numerically odd — agent name; see `agent_identity`'s own doc.
        assert_eq!(
            agent_identity(&serde_json::json!({"agent": {"agent": 42}})),
            None
        );
    }

    #[test]
    fn agent_identity_is_none_for_a_null_result() {
        // `interpret_output` maps an empty-but-successful stdout to `Ok(Value::Null)` — confirm
        // that shape is treated the same as any other response with no identity, not a panic.
        assert_eq!(agent_identity(&serde_json::Value::Null), None);
    }
}
