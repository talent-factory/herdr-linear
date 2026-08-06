//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `start_implementation`) to open a tab, start an
//! agent, wait for it to become ready, and inject text. The subprocess-spawning half is
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
        if let Some(agent_name_taken) = parse_agent_name_taken_error(error_obj) {
            return Err(agent_name_taken);
        }

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

/// Extract [`Error::AgentNameTaken`] from a herdr error body's `error.code`/`error.candidates`
/// fields (TF-590), if present. Split out from [`interpret_output`] so the one case
/// `agent_start`'s retry logic cares about is unit-testable in isolation, the same way
/// [`is_missing_result_response`]'s companion case is.
fn parse_agent_name_taken_error(error_obj: Option<&Value>) -> Option<Error> {
    let code = error_obj?.get("code")?.as_str()?;
    if code != "agent_name_taken" {
        return None;
    }
    let candidates = error_obj
        .and_then(|e| e.get("candidates"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(Error::AgentNameTaken { candidates })
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
/// argument (everything routed through [`run`]: `agent_list`, `agent_start`, `tab_rename`,
/// `agent_send`). Without this, a hung `herdr` daemon blocks the single-threaded TUI's event
/// loop indefinitely — `agent_wait` is the exception, since it computes its own call-specific
/// bound in [`agent_wait`] instead of using this constant.
const DEFAULT_CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Run a `herdr` CLI subcommand, bounded by `call_timeout`, returning the parsed `result` field
/// on success. See [`interpret_output`] for the success/failure mapping.
async fn run_with_timeout(herdr_bin: &str, args: &[&str], call_timeout: Duration) -> Result<Value> {
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
    )
}

/// [`run_with_timeout`] bounded by [`DEFAULT_CLI_TIMEOUT`] — used by every `herdr` subcommand
/// except `agent_wait`.
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    run_with_timeout(herdr_bin, args, DEFAULT_CLI_TIMEOUT).await
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

/// Max retries [`agent_start`] makes after its initial call when herdr reports
/// `agent_name_taken` (TF-590, see [`Error::AgentNameTaken`]) before giving up and reporting
/// the collision to the caller. Bounds retries even against a degenerate/huge `candidates`
/// list from herdr, since blindly working through an unbounded list could hang the flow.
const AGENT_START_NAME_TAKEN_MAX_RETRIES: u32 = 2;

/// Pick the next agent name to retry [`agent_start`] with after an `agent_name_taken`
/// collision, given herdr's suggested `candidates` (from the error that just failed) and how
/// many retries have already been attempted. Pure — no I/O — so the retry decision is
/// unit-testable without spawning a process, the same way `agent_wait`'s
/// [`next_retry_budget_ms`] is. Returns `None` once the retry budget
/// ([`AGENT_START_NAME_TAKEN_MAX_RETRIES`]) is exhausted or herdr reported no candidates to
/// try, in which case the caller gives up and reports the collision.
fn next_name_taken_retry(candidates: &[String], attempt: u32) -> Option<&str> {
    if attempt >= AGENT_START_NAME_TAKEN_MAX_RETRIES {
        return None;
    }
    candidates.first().map(String::as_str)
}

/// `herdr agent start <name> --cwd <cwd> --focus -- <argv...>` — starts `name` (used by herdr
/// for its own agent-status tracking) running `argv` in a fresh, focused tab at `cwd`.
///
/// If herdr rejects `name` with `agent_name_taken` (TF-590 — e.g. because a previous issue's
/// agent tab is still running under a name that collides with this one), retries
/// automatically with one of herdr's suggested `candidates` instead of surfacing the raw
/// collision to the caller — see `next_name_taken_retry`.
pub async fn agent_start(
    herdr_bin: &str,
    name: &str,
    cwd: &Path,
    argv: &[String],
) -> Result<AgentStarted> {
    let cwd_str = cwd.to_string_lossy().to_string();
    let mut attempt_name = name.to_string();
    let mut attempt = 0;

    loop {
        let mut args: Vec<&str> = vec![
            "agent",
            "start",
            &attempt_name,
            "--cwd",
            &cwd_str,
            "--focus",
            "--",
        ];
        for a in argv {
            args.push(a.as_str());
        }

        match run(herdr_bin, &args).await {
            Ok(result) => return parse_agent_started(&result),
            Err(Error::AgentNameTaken { candidates }) => {
                match next_name_taken_retry(&candidates, attempt) {
                    Some(candidate) => {
                        attempt_name = candidate.to_string();
                        attempt += 1;
                    }
                    None => {
                        return Err(Error::Internal(format!(
                            "agent_name_taken: `{name}` and every retry candidate herdr suggested \
                         are already in use (last tried: `{attempt_name}`)"
                        )));
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// `herdr tab rename <tab_id> <label>`.
pub async fn tab_rename(herdr_bin: &str, tab_id: &TabId, label: &str) -> Result<()> {
    run(herdr_bin, &["tab", "rename", tab_id.as_str(), label])
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

    #[test]
    fn interpret_output_maps_agent_name_taken_to_a_structured_error_with_candidates() {
        let result = interpret_output(
            "herdr agent start hr --cwd . --focus -- zsh",
            false,
            r#"{"error":{"code":"agent_name_taken","message":"agent name hr is already used","candidates":["hr-2","hr-3"]}}"#,
            "",
        );

        match result.unwrap_err() {
            Error::AgentNameTaken { candidates } => {
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
        );

        match result.unwrap_err() {
            Error::AgentNameTaken { candidates } => assert!(candidates.is_empty()),
            other => panic!("expected AgentNameTaken, got {other:?}"),
        }
    }

    #[test]
    fn interpret_output_does_not_treat_other_error_codes_as_agent_name_taken() {
        let result = interpret_output(
            "herdr agent send bogus hi",
            false,
            r#"{"error":{"code":"no_such_pane","message":"no such pane"}}"#,
            "",
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no such pane"), "unexpected message: {err}");
    }

    #[test]
    fn next_name_taken_retry_returns_the_first_candidate_within_budget() {
        let candidates = vec!["hr-2".to_string(), "hr-3".to_string()];

        assert_eq!(next_name_taken_retry(&candidates, 0), Some("hr-2"));
    }

    #[test]
    fn next_name_taken_retry_stops_once_the_retry_cap_is_reached() {
        let candidates = vec!["hr-2".to_string()];

        assert_eq!(
            next_name_taken_retry(&candidates, AGENT_START_NAME_TAKEN_MAX_RETRIES),
            None
        );
    }

    #[test]
    fn next_name_taken_retry_returns_none_when_herdr_reports_no_candidates() {
        assert_eq!(next_name_taken_retry(&[], 0), None);
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
