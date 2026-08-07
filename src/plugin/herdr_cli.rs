//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `implement_one`, shared by both the single- and
//! multi-issue callers) to open a tab, start an agent, wait for it to become ready, and inject
//! text. The subprocess-spawning half is
//! deliberately untested at this layer — same status as the existing `open::that(url)` call for
//! the `o` key; see docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for why. The
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

/// Result of a successful `herdr agent start` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStarted {
    pub pane_id: PaneId,
    pub tab_id: TabId,
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

/// Pure interpretation of a `herdr` CLI invocation's raw output into the `Result` `run` returns.
/// Maps a non-zero exit, a top-level `{"error": {"message": ...}}` response (checked
/// independently of the exit code — a future protocol change that reports failure via body
/// alone, exit 0, must not be misread as success), or unparseable JSON to `Error::Internal` with
/// the CLI's own error message (or raw stderr/stdout as a fallback) so failures are always
/// actionable in the status banner they end up in. Split out from `run` so this logic — the part
/// that actually decides success vs. failure — is unit-testable without spawning a process.
///
/// When `check_agent_name_taken` is set, an `error.code == "agent_name_taken"` body is mapped to
/// [`Error::AgentNameTaken`] instead of the generic `Error::Internal` path (see
/// [`parse_agent_name_taken_error`]) — only [`agent_start`] (via [`run_for_agent_start`]) passes
/// `true`. Scoped this way rather than checked unconditionally for every `herdr` subcommand
/// because `agent_name_taken` is meaningful only in the context of a `agent start` call; if any
/// other subcommand (`tab_create`, `agent_wait`, ...) ever received a response reusing that same
/// code, treating it as the same structured error would surface the wrong candidates/remedy
/// out of context, with no retry logic actually applying.
fn interpret_output(
    command_desc: &str,
    status_success: bool,
    stdout: &str,
    stderr: &str,
    check_agent_name_taken: bool,
) -> Result<Value> {
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();

    let error_obj = parsed.as_ref().and_then(|v| v.get("error"));

    // Checked independently of `status_success`/`error.message` presence, and before the
    // generic failure gate below: herdr could in principle report an `agent_name_taken`
    // collision with a clean exit and no `message` field at all (see
    // `parse_agent_name_taken_error`'s message fallback, added for exactly that case), and
    // gating this on "is this a failure at all" would let such a response slip through as a
    // false success instead of the collision it actually is.
    if check_agent_name_taken {
        if let Some(agent_name_taken) = parse_agent_name_taken_error(error_obj) {
            return Err(agent_name_taken);
        }
    }

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

/// Extract [`Error::AgentNameTaken`] from a herdr error body's `error.code`/`error.message`/
/// `error.candidates` fields (TF-590), if present. Split out from [`interpret_output`] so the
/// one case `agent_start`'s retry logic cares about is unit-testable in isolation, the same way
/// [`is_missing_result_response`]'s companion case is.
///
/// `message` falls back to a generic notice if herdr's body omits it (shouldn't happen in
/// practice, but the field is read the same optional way `candidates` already was). If
/// `candidates` is present but isn't a JSON array at all, this logs a `tracing::warn!` and treats
/// it the same as "no candidates" (empty). If it *is* an array but contains non-string entries,
/// only those entries are dropped (also with a `tracing::warn!`) — any remaining valid string
/// candidates are still returned and still available to the retry loop, so this is a partial
/// list in that case, not an empty one. Either way, a malformed response is a herdr protocol
/// surprise worth knowing about (via `$HERDR_LINEAR_LOG_FILE`, see `main.rs::init_tracing`), not
/// silently indistinguishable from herdr legitimately having nothing (more) to suggest.
fn parse_agent_name_taken_error(error_obj: Option<&Value>) -> Option<Error> {
    let code = error_obj?.get("code")?.as_str()?;
    if code != "agent_name_taken" {
        return None;
    }
    let message = error_obj
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "agent name already in use".to_string());
    let candidates_field = error_obj.and_then(|e| e.get("candidates"));
    let candidates = match candidates_field {
        None => Vec::new(),
        Some(value) => match value.as_array() {
            Some(arr) => {
                let candidates: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                if candidates.len() != arr.len() {
                    tracing::warn!(
                        "herdr's agent_name_taken response had a `candidates` array with \
                         non-string entries; dropped {} of {}",
                        arr.len() - candidates.len(),
                        arr.len()
                    );
                }
                candidates
            }
            None => {
                tracing::warn!(
                    "herdr's agent_name_taken response had a `candidates` field that wasn't a \
                     JSON array; treating as no candidates"
                );
                Vec::new()
            }
        },
    };
    Some(Error::AgentNameTaken {
        message,
        candidates,
    })
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
/// argument (everything routed through [`run`]: `agent_list`, `tab_create`, `agent_start`,
/// `agent_send`, `pane_close`). Without this, a hung `herdr` daemon blocks the single-threaded
/// TUI's event loop indefinitely — `agent_wait` is the exception, since it computes its own
/// call-specific bound in [`agent_wait`] instead of using this constant.
const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Run a `herdr` CLI subcommand, bounded by `call_timeout`, returning the parsed `result` field
/// on success. See [`interpret_output`] for the success/failure mapping.
async fn run_with_timeout(
    herdr_bin: &str,
    args: &[&str],
    call_timeout: Duration,
    check_agent_name_taken: bool,
) -> Result<Value> {
    let command_desc = format!("{herdr_bin} {}", args.join(" "));

    let output = tokio::time::timeout(call_timeout, Command::new(herdr_bin).args(args).output())
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
        check_agent_name_taken,
    )
}

/// [`run_with_timeout`] bounded by [`DEFAULT_CLI_TIMEOUT`], with `agent_name_taken` mapping
/// switched off — used by every `herdr` subcommand except `agent_wait` (which has its own
/// budget) and `agent_start` (see [`run_for_agent_start`]).
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(herdr_bin, args, DEFAULT_CLI_TIMEOUT, false).await
}

/// [`run_with_timeout`] bounded by [`DEFAULT_CLI_TIMEOUT`], with `agent_name_taken` mapping
/// switched on — used only by [`agent_start`], the one call site that knows how to retry an
/// `agent_name_taken` collision (see [`interpret_output`]'s docs for why this is opt-in rather
/// than global).
async fn run_for_agent_start(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(herdr_bin, args, DEFAULT_CLI_TIMEOUT, true).await
}

/// `herdr agent list` — the raw JSON text of the `result` field, for
/// [`crate::plugin::implement::resolve_preferred_agent`] to parse.
pub async fn agent_list(herdr_bin: &str) -> Result<String> {
    let result = run(herdr_bin, &["agent", "list"]).await?;
    Ok(result.to_string())
}

/// Extract [`AgentStarted`] from a `herdr agent start` call's already-unwrapped `result` value.
/// Split out from [`agent_start`] so the part that can actually be wrong — a schema change or a
/// herdr regression in the response shape — is unit-testable without spawning a process, the
/// same way [`interpret_output`] is split out of [`run`].
fn parse_agent_started(result: &Value) -> Result<AgentStarted> {
    let pane_id = result
        .get("agent")
        .and_then(|a| a.get("pane_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Internal("agent.start response missing agent.pane_id".to_string()))?
        .to_string();
    let tab_id = result
        .get("agent")
        .and_then(|a| a.get("tab_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Internal("agent.start response missing agent.tab_id".to_string()))?
        .to_string();

    Ok(AgentStarted {
        pane_id: PaneId(pane_id),
        tab_id: TabId(tab_id),
    })
}

/// Extract [`TabCreated`] from a `herdr tab create` call's already-unwrapped `result` value.
/// Split out from [`tab_create`] for the same testability reason as [`parse_agent_started`].
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

/// Max retries [`agent_start`] makes after its initial call when herdr reports
/// `agent_name_taken` (TF-590, see [`Error::AgentNameTaken`]) before giving up and reporting
/// the collision to the caller. Bounds how many round-trips to herdr the retry loop makes,
/// rather than how far into `candidates` it walks — [`next_name_taken_retry`] walks the full
/// list each round looking for a name not yet tried, so an unusually long `candidates` list
/// doesn't by itself cost extra round-trips, only an unusually long run of already-tried names
/// would.
const AGENT_START_NAME_TAKEN_MAX_RETRIES: u32 = 2;

/// Pick the next agent name to retry [`agent_start`] with after an `agent_name_taken`
/// collision, given herdr's suggested `candidates` (from the error that just failed), the set
/// of names already tried this call (`tried`), and how many retries have already been
/// attempted. Skips any candidate already in `tried` so a herdr response that re-suggests an
/// already-rejected name doesn't silently burn a retry making no progress. Pure — no I/O — so
/// the retry decision is unit-testable without spawning a process, the same way `agent_wait`'s
/// [`next_retry_budget_ms`] is. Returns `None` once the retry budget
/// ([`AGENT_START_NAME_TAKEN_MAX_RETRIES`]) is exhausted or none of `candidates` is a name that
/// hasn't already been tried, in which case the caller gives up and reports the collision.
fn next_name_taken_retry<'a>(
    candidates: &'a [String],
    tried: &std::collections::HashSet<String>,
    attempt: u32,
) -> Option<&'a str> {
    if attempt >= AGENT_START_NAME_TAKEN_MAX_RETRIES {
        return None;
    }
    candidates
        .iter()
        .map(String::as_str)
        .find(|candidate| !tried.contains(*candidate))
}

/// `herdr agent start <name> --cwd <cwd> --tab <tab> --focus -- <argv...>` — starts `name` (used
/// by herdr for its own agent-status tracking) running `argv` at `cwd`, explicitly placed inside
/// `tab` (created via [`tab_create`]) rather than trusting herdr's own default placement — see
/// docs/superpowers/specs/2026-08-06-guaranteed-tab-per-issue-design.md for why an implicit
/// placement previously let one issue's agent land as a split inside a different, already-running
/// issue's tab. There is deliberately no variant of this function that omits `tab`.
///
/// If herdr rejects `name` with `agent_name_taken` (TF-590 — e.g. because a previous issue's
/// agent tab is still running under a name that collides with this one), retries automatically
/// — up to `AGENT_START_NAME_TAKEN_MAX_RETRIES` times — with one of herdr's suggested
/// `candidates` instead of surfacing the raw collision to the caller — see
/// `next_name_taken_retry`. If every candidate is exhausted or already tried, gives up and
/// returns an [`Error::Internal`] that carries herdr's own message from the *last* collision
/// plus a concrete remedy, distinguishing "herdr suggested nothing" from "every suggestion was
/// already tried" so the message doesn't overstate what was actually attempted.
pub async fn agent_start(
    herdr_bin: &str,
    name: &str,
    cwd: &Path,
    tab: &TabId,
    argv: &[String],
) -> Result<AgentStarted> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut attempt_name = name.to_string();
    let mut attempt = 0;
    let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
    tried.insert(attempt_name.clone());

    loop {
        let mut args: Vec<&str> = vec![
            "agent",
            "start",
            &attempt_name,
            "--cwd",
            &cwd_str,
            "--tab",
            tab.as_str(),
            "--focus",
            "--",
        ];
        for a in argv {
            args.push(a.as_str());
        }

        match run_for_agent_start(herdr_bin, &args).await {
            Ok(result) => return parse_agent_started(&result),
            Err(Error::AgentNameTaken {
                message,
                candidates,
            }) => match next_name_taken_retry(&candidates, &tried, attempt) {
                Some(candidate) => {
                    tracing::debug!(
                        "agent_start: name {attempt_name:?} taken ({message}), retrying with \
                             {candidate:?} (attempt {})",
                        attempt + 1
                    );
                    attempt_name = candidate.to_string();
                    tried.insert(attempt_name.clone());
                    attempt += 1;
                }
                None => {
                    // `next_name_taken_retry` returns `None` for two different reasons that must
                    // not be conflated in the message: the retry *budget* ran out (some of
                    // `candidates` were never attempted), or every candidate herdr has ever
                    // suggested is already in `tried`. Reporting an untried candidate as "already
                    // tried" would send the user chasing a name that was never actually attempted.
                    let untried: Vec<&str> = candidates
                        .iter()
                        .map(String::as_str)
                        .filter(|c| !tried.contains(*c))
                        .collect();
                    let remedy = if candidates.is_empty() {
                        "herdr suggested no alternative name".to_string()
                    } else if untried.is_empty() {
                        format!(
                            "the suggested alternative{} ({}) {} already tried",
                            if candidates.len() == 1 { "" } else { "s" },
                            candidates.join(", "),
                            if candidates.len() == 1 { "was" } else { "were" }
                        )
                    } else {
                        format!(
                            "gave up after {attempt} retries; herdr's suggested alternative{} \
                                 ({}) {} not yet tried",
                            if untried.len() == 1 { "" } else { "s" },
                            untried.join(", "),
                            if untried.len() == 1 { "was" } else { "were" }
                        )
                    };
                    return Err(Error::Internal(format!(
                        "agent_name_taken: {message} — {remedy}; close the other agent tab \
                             or wait for it to finish, then retry"
                    )));
                }
            },
            Err(err) => return Err(err),
        }
    }
}

/// `herdr pane close <pane_id>`. Used to close the now-redundant root pane [`tab_create`] leaves
/// behind once [`agent_start`] has placed the real agent pane into the same tab (`agent_start`
/// never replaces a tab's existing panes, only splits alongside them).
pub async fn pane_close(herdr_bin: &str, pane_id: &PaneId) -> Result<()> {
    run(herdr_bin, &["pane", "close", pane_id.as_str()])
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

/// `herdr agent wait <pane_id> --status <status> --timeout <timeout_ms>`. Retries, within the
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
                "--status",
                status,
                "--timeout",
                &timeout_str,
            ],
            call_timeout,
            false,
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

/// `herdr agent send <pane_id> <text>`.
pub async fn agent_send(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "send", pane_id.as_str(), text])
        .await
        .map(|_| ())
}

/// Extract the rendered pane text from a `herdr agent read` call's already-unwrapped `result`
/// value. Split out from [`agent_read`] for the same testability reason as
/// [`parse_agent_started`].
fn parse_agent_read(result: &Value) -> Result<String> {
    result
        .get("read")
        .and_then(|r| r.get("text"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| Error::Internal("agent.read response missing read.text".to_string()))
}

/// `herdr agent read <pane_id> --source <source> --lines <lines>` — the pane's rendered
/// terminal text. Used by `main.rs`'s `send_prompt_until_visible` to confirm an [`agent_send`]
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
        let result = interpret_output(
            "herdr agent list",
            true,
            r#"{"result":{"agents":[]}}"#,
            "",
            false,
        );

        assert_eq!(result.unwrap(), serde_json::json!({"agents": []}));
    }

    #[test]
    fn interpret_output_errors_on_non_zero_exit_with_a_json_error_body() {
        let result = interpret_output(
            "herdr agent send bogus hi",
            false,
            r#"{"error":{"message":"no such pane"}}"#,
            "",
            false,
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
            false,
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out waiting for idle"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_errors_on_non_zero_exit_with_no_stderr_falling_back_to_stdout() {
        let result = interpret_output("herdr bogus", false, "unknown option: --bogus\n", "", false);

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown option: --bogus"),
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
            false,
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no such pane"), "unexpected message: {err}");
    }

    #[test]
    fn interpret_output_errors_on_unparseable_json_with_a_zero_exit() {
        let result = interpret_output("herdr agent list", true, "not json", "", false);

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unparseable output"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn interpret_output_errors_when_the_result_field_is_missing() {
        let result = interpret_output(
            "herdr agent list",
            true,
            r#"{"id":"cli:agent:list"}"#,
            "",
            false,
        );

        assert!(result.is_err());
    }

    #[test]
    fn is_missing_result_response_matches_the_missing_result_error() {
        let err = interpret_output(
            "herdr agent wait wY:p1Q --status idle",
            true,
            r#"{"id":"x"}"#,
            "",
            false,
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
            false,
        )
        .unwrap_err();
        let unparseable =
            interpret_output("herdr agent list", true, "not json", "", false).unwrap_err();
        let spawn_failed = Error::Internal("Failed to run `herdr`: no such file".to_string());

        assert!(!is_missing_result_response(&failed));
        assert!(!is_missing_result_response(&unparseable));
        assert!(!is_missing_result_response(&spawn_failed));
    }

    #[test]
    fn interpret_output_maps_agent_name_taken_to_a_structured_error_with_candidates() {
        let result = interpret_output(
            "herdr agent start hr --cwd . --focus -- zsh",
            false,
            r#"{"error":{"code":"agent_name_taken","message":"agent name hr is already used","candidates":["hr-2","hr-3"]}}"#,
            "",
            true,
        );

        match result.unwrap_err() {
            Error::AgentNameTaken {
                message,
                candidates,
            } => {
                assert_eq!(message, "agent name hr is already used");
                assert_eq!(candidates, vec!["hr-2".to_string(), "hr-3".to_string()])
            }
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn interpret_output_maps_agent_name_taken_with_no_candidates_to_an_empty_list() {
        let result = interpret_output(
            "herdr agent start hr --cwd . --focus -- zsh",
            false,
            r#"{"error":{"code":"agent_name_taken","message":"agent name hr is already used"}}"#,
            "",
            true,
        );

        match result.unwrap_err() {
            Error::AgentNameTaken { candidates, .. } => assert!(candidates.is_empty()),
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn interpret_output_maps_agent_name_taken_even_on_a_zero_exit_with_no_message_field() {
        // herdr could in principle report an `agent_name_taken` collision with a successful exit
        // code and no `message` field at all — `agent_name_taken` detection must not depend on
        // the outer "is this a failure at all" gate (`!status_success || error_message.is_some()`),
        // since that gate is exactly what a clean-exit, message-less error body slips past.
        let result = interpret_output(
            "herdr agent start hr --cwd . --focus -- zsh",
            true,
            r#"{"error":{"code":"agent_name_taken","candidates":["hr-2"]}}"#,
            "",
            true,
        );

        match result.unwrap_err() {
            Error::AgentNameTaken { candidates, .. } => {
                assert_eq!(candidates, vec!["hr-2".to_string()])
            }
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn interpret_output_ignores_agent_name_taken_when_not_checking_for_it() {
        // TF-590 follow-up: `interpret_output` only maps `agent_name_taken` when the caller
        // (only `agent_start`, via `run_for_agent_start`) opts in — every other `herdr`
        // subcommand must fall through to the generic error path even if herdr ever reused
        // this code for a response unrelated to `agent start`.
        let result = interpret_output(
            "herdr tab create --label ENG-1",
            false,
            r#"{"error":{"code":"agent_name_taken","message":"agent name hr is already used","candidates":["hr-2"]}}"#,
            "",
            false,
        );

        match result.unwrap_err() {
            Error::AgentNameTaken { .. } => panic!("agent_name_taken should not be checked here"),
            other => assert!(
                other.to_string().contains("agent name hr is already used"),
                "unexpected message: {other}"
            ),
        }
    }

    #[test]
    fn interpret_output_does_not_treat_other_error_codes_as_agent_name_taken() {
        let result = interpret_output(
            "herdr agent send bogus hi",
            false,
            r#"{"error":{"code":"no_such_pane","message":"no such pane"}}"#,
            "",
            true,
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no such pane"), "unexpected message: {err}");
    }

    #[test]
    fn next_name_taken_retry_returns_the_first_untried_candidate_within_budget() {
        let candidates = vec!["hr-2".to_string(), "hr-3".to_string()];
        let tried = std::collections::HashSet::new();

        assert_eq!(next_name_taken_retry(&candidates, &tried, 0), Some("hr-2"));
    }

    #[test]
    fn next_name_taken_retry_skips_candidates_already_tried() {
        // The whole point of tracking `tried`: a herdr response that re-suggests a name we
        // already know is taken must not be picked again.
        let candidates = vec!["hr-2".to_string(), "hr-3".to_string()];
        let mut tried = std::collections::HashSet::new();
        tried.insert("hr-2".to_string());

        assert_eq!(next_name_taken_retry(&candidates, &tried, 0), Some("hr-3"));
    }

    #[test]
    fn next_name_taken_retry_returns_none_when_every_candidate_was_already_tried() {
        let candidates = vec!["hr-2".to_string(), "hr-3".to_string()];
        let mut tried = std::collections::HashSet::new();
        tried.insert("hr-2".to_string());
        tried.insert("hr-3".to_string());

        assert_eq!(next_name_taken_retry(&candidates, &tried, 0), None);
    }

    #[test]
    fn next_name_taken_retry_stops_once_the_retry_cap_is_reached() {
        let candidates = vec!["hr-2".to_string()];
        let tried = std::collections::HashSet::new();

        assert_eq!(
            next_name_taken_retry(&candidates, &tried, AGENT_START_NAME_TAKEN_MAX_RETRIES),
            None
        );
    }

    #[test]
    fn next_name_taken_retry_returns_none_when_herdr_reports_no_candidates() {
        let tried = std::collections::HashSet::new();
        assert_eq!(next_name_taken_retry(&[], &tried, 0), None);
    }

    #[test]
    fn next_name_taken_retry_still_works_at_a_mid_budget_attempt() {
        // Regression guard: `attempt` only gates the budget check (`attempt >=
        // AGENT_START_NAME_TAKEN_MAX_RETRIES`), it isn't used to index into `candidates` — a
        // retry partway through the budget (not the first, not yet at the cap) must still pick
        // a valid untried candidate.
        let candidates = vec!["hr-2".to_string(), "hr-3".to_string()];
        let mut tried = std::collections::HashSet::new();
        tried.insert("hr-2".to_string());

        assert_eq!(next_name_taken_retry(&candidates, &tried, 1), Some("hr-3"));
    }

    #[test]
    fn parse_agent_name_taken_error_logs_and_drops_a_non_array_candidates_field() {
        let error_obj = serde_json::json!({
            "code": "agent_name_taken",
            "message": "agent name hr is already used",
            "candidates": "hr-2",
        });

        match parse_agent_name_taken_error(Some(&error_obj)).unwrap() {
            Error::AgentNameTaken { candidates, .. } => assert!(candidates.is_empty()),
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_name_taken_error_drops_non_string_candidate_entries() {
        let error_obj = serde_json::json!({
            "code": "agent_name_taken",
            "message": "agent name hr is already used",
            "candidates": ["hr-2", 3, "hr-4"],
        });

        match parse_agent_name_taken_error(Some(&error_obj)).unwrap() {
            Error::AgentNameTaken { candidates, .. } => {
                assert_eq!(candidates, vec!["hr-2".to_string(), "hr-4".to_string()])
            }
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_name_taken_error_falls_back_to_a_generic_message_when_absent() {
        let error_obj = serde_json::json!({"code": "agent_name_taken"});

        match parse_agent_name_taken_error(Some(&error_obj)).unwrap() {
            Error::AgentNameTaken { message, .. } => {
                assert_eq!(message, "agent name already in use")
            }
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    /// Writes an executable fake `herdr` shell script (`#!/bin/sh` — Unix only, matching the
    /// project's own `sh -i`/`/bin/bash` assumptions) into a fresh [`tempfile::TempDir`] and
    /// returns both, so `agent_start`'s subprocess-spawning retry loop can be exercised
    /// end-to-end without a real `herdr` daemon. `agent_start` takes `herdr_bin` directly as a
    /// parameter (see `herdr_bin()`'s `$HERDR_BIN_PATH` override, which this mirrors), so the
    /// script's path can be passed straight in — no env var indirection needed. The `TempDir`
    /// must be kept alive by the caller for the duration of the test (dropping it deletes the
    /// script).
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
    async fn agent_start_retries_with_herdrs_suggested_candidate_and_then_succeeds() {
        // $3 is the `<name>` positional in `agent start <name> --cwd ... --focus -- ...`.
        let (_dir, script) = write_fake_herdr_script(
            r#"
name="$3"
if [ "$name" = "hr" ]; then
  echo '{"error":{"code":"agent_name_taken","message":"agent name hr is already used","candidates":["hr-2"]}}'
  exit 1
elif [ "$name" = "hr-2" ]; then
  echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t1"}}}'
  exit 0
else
  echo "{\"error\":{\"message\":\"unexpected name: $name\"}}"
  exit 1
fi
"#,
        );

        let started = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect("agent_start should retry with herdr's suggested candidate and then succeed");

        assert_eq!(started.pane_id.as_str(), "p1");
        assert_eq!(started.tab_id.as_str(), "t1");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_gives_up_once_the_retry_budget_is_exhausted() {
        // Always collides, and always re-suggests the same two candidates regardless of which
        // name was just tried — this is exactly the "herdr keeps re-suggesting an already-tried
        // name" scenario `next_name_taken_retry`'s `tried` tracking exists for.
        let (_dir, script) = write_fake_herdr_script(
            r#"
name="$3"
echo "{\"error\":{\"code\":\"agent_name_taken\",\"message\":\"agent name $name is already used\",\"candidates\":[\"hr-2\",\"hr-3\"]}}"
exit 1
"#,
        );

        let err = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect_err("agent_start should give up once every candidate has been tried");

        let message = err.to_string();
        assert!(
            message.contains("agent_name_taken"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("already tried"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("close the other agent tab"),
            "give-up message should include a remedy: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_gives_up_immediately_when_herdr_suggests_no_alternative() {
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"code":"agent_name_taken","message":"agent name hr is already used"}}'
exit 1
"#,
        );

        let err = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect_err("agent_start should give up when herdr offers no alternative");

        let message = err.to_string();
        assert!(
            message.contains("no alternative name"),
            "unexpected message: {message}"
        );
        assert!(
            !message.contains("already tried"),
            "message shouldn't claim alternatives were tried when none were offered: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_does_not_claim_an_untried_candidate_was_already_tried_when_the_budget_runs_out_first(
    ) {
        // Three distinct candidates every time, but AGENT_START_NAME_TAKEN_MAX_RETRIES (2) means
        // the retry *budget* is exhausted after two retries, before the loop ever gets to try the
        // third candidate. Regression test for the give-up message previously claiming every
        // candidate herdr ever suggested (including ones the loop never attempted) was "already
        // tried".
        let (_dir, script) = write_fake_herdr_script(
            r#"
name="$3"
echo "{\"error\":{\"code\":\"agent_name_taken\",\"message\":\"agent name $name is already used\",\"candidates\":[\"hr-2\",\"hr-3\",\"hr-4\"]}}"
exit 1
"#,
        );

        let err = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect_err("agent_start should give up once the retry budget is exhausted");

        let message = err.to_string();
        // hr-4 is never attempted: attempt 0 tries hr-2, attempt 1 tries hr-3, and the budget
        // (2 retries) is exhausted before a third retry would try hr-4.
        assert!(
            !message.contains("hr-4) were already tried")
                && !message.contains("hr-2, hr-3, hr-4) were already tried"),
            "hr-4 was never attempted and must not be reported as already tried: {message}"
        );
        assert!(
            message.contains("hr-4"),
            "the untried candidate should still be surfaced, just not as \"already tried\": {message}"
        );
        assert!(
            message.contains("close the other agent tab"),
            "give-up message should include a remedy: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_retries_twice_before_succeeding_on_the_final_attempt() {
        // Rejects the first two names and succeeds on the third — exercises whether `tried`/
        // `attempt` actually accumulate correctly across multiple loop iterations, not just a
        // single retry (the exact kind of wiring bug TF-590's own fixup commit addressed).
        let (_dir, script) = write_fake_herdr_script(
            r#"
name="$3"
if [ "$name" = "hr" ]; then
  echo '{"error":{"code":"agent_name_taken","message":"agent name hr is already used","candidates":["hr-2"]}}'
  exit 1
elif [ "$name" = "hr-2" ]; then
  echo '{"error":{"code":"agent_name_taken","message":"agent name hr-2 is already used","candidates":["hr-3"]}}'
  exit 1
elif [ "$name" = "hr-3" ]; then
  echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t1"}}}'
  exit 0
else
  echo "{\"error\":{\"message\":\"unexpected name: $name\"}}"
  exit 1
fi
"#,
        );

        let started = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect("agent_start should retry twice and succeed on the third attempt");

        assert_eq!(started.pane_id.as_str(), "p1");
        assert_eq!(started.tab_id.as_str(), "t1");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_succeeds_on_the_first_attempt_without_retrying() {
        // Guards the `Ok(result) => return parse_agent_started(&result)` arm against a future
        // refactor of the retry loop accidentally requiring at least one retry.
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t1"}}}'
exit 0
"#,
        );

        let started = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect("agent_start should succeed on the very first attempt");

        assert_eq!(started.pane_id.as_str(), "p1");
        assert_eq!(started.tab_id.as_str(), "t1");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_passes_the_requested_tab_id_via_the_tab_flag() {
        // TF-579: the whole point of this parameter is that `agent_start` explicitly places the
        // agent into a pre-created tab rather than trusting herdr's default placement. Assert on
        // the actual argv, not just the returned `AgentStarted` — a dropped/misplaced/mis-valued
        // `--tab` flag would reintroduce TF-579's bug while every other test here still passes.
        let capture_dir = tempfile::tempdir().unwrap();
        let args_file = capture_dir.path().join("args.txt");
        let (_dir, script) = write_fake_herdr_script(&format!(
            r#"
printf '%s\n' "$@" > "{}"
echo '{{"result":{{"agent":{{"pane_id":"p1","tab_id":"t1"}}}}}}'
exit 0
"#,
            args_file.display()
        ));

        agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect("agent_start should succeed");

        let captured = std::fs::read_to_string(&args_file).unwrap();
        let args: Vec<&str> = captured.lines().collect();
        assert_eq!(
            args,
            vec!["agent", "start", "hr", "--cwd", "/tmp", "--tab", "t0", "--focus", "--", "zsh"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_start_propagates_a_non_agent_name_taken_error_without_retrying() {
        // Guards the `Err(err) => return Err(err)` arm, new code introduced by wrapping the
        // single `herdr agent start` call in a loop: a genuine, non-collision failure (bad
        // `--cwd`, herdr not running, ...) must return immediately, not enter the retry loop.
        let (_dir, script) = write_fake_herdr_script(
            r#"
echo '{"error":{"message":"no such pane"}}'
exit 1
"#,
        );

        let err = agent_start(
            script.to_str().unwrap(),
            "hr",
            Path::new("/tmp"),
            &TabId("t0".to_string()),
            &["zsh".to_string()],
        )
        .await
        .expect_err("agent_start should propagate a non-collision error immediately");

        let message = err.to_string();
        assert!(
            message.contains("no such pane"),
            "unexpected message: {message}"
        );
        assert!(
            !message.contains("agent_name_taken"),
            "a plain error must not be reported as a name collision: {message}"
        );
    }

    #[test]
    fn parse_agent_started_extracts_pane_id_and_tab_id() {
        let result = serde_json::json!({"agent": {"pane_id": "wY:p3", "tab_id": "wY:tW"}});

        let started = parse_agent_started(&result).unwrap();

        assert_eq!(started.pane_id.as_str(), "wY:p3");
        assert_eq!(started.tab_id.as_str(), "wY:tW");
    }

    #[test]
    fn parse_agent_started_errors_when_pane_id_is_missing() {
        let result = serde_json::json!({"agent": {"tab_id": "wY:tW"}});

        let err = parse_agent_started(&result).unwrap_err().to_string();

        assert!(err.contains("agent.pane_id"), "unexpected message: {err}");
    }

    #[test]
    fn parse_agent_started_errors_when_tab_id_is_missing() {
        let result = serde_json::json!({"agent": {"pane_id": "wY:p3"}});

        let err = parse_agent_started(&result).unwrap_err().to_string();

        assert!(err.contains("agent.tab_id"), "unexpected message: {err}");
    }

    #[test]
    fn parse_agent_started_errors_when_the_agent_object_is_missing_entirely() {
        let result = serde_json::json!({"id": "cli:agent:start"});

        assert!(parse_agent_started(&result).is_err());
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
}
