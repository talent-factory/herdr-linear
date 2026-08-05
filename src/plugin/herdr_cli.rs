//! Thin subprocess wrapper around the `herdr` CLI's JSON socket protocol, used by the
//! "implement this issue" flow (`main.rs`'s `start_implementation`) to open a tab, start an
//! agent, wait for it to become ready, and inject text. The subprocess-spawning half is
//! deliberately untested at this layer — same status as the existing `open::that(url)` call for
//! the `o` key; see docs/superpowers/specs/2026-08-05-implement-on-enter-design.md for why. The
//! response-interpretation half (`interpret_output`) is pure and unit-tested below.

use crate::error::Error;
use crate::Result;
use serde_json::Value;
use std::path::Path;
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

    let error_message = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
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
        .ok_or_else(|| Error::Internal(format!("`{command_desc}` had no `result` field")))
}

/// Run a `herdr` CLI subcommand, returning the parsed `result` field on success. See
/// [`interpret_output`] for the success/failure mapping.
async fn run(herdr_bin: &str, args: &[&str]) -> Result<Value> {
    let output = Command::new(herdr_bin)
        .args(args)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to run `{herdr_bin}`: {e}")))?;

    let command_desc = format!("{herdr_bin} {}", args.join(" "));
    interpret_output(
        &command_desc,
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
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

/// `herdr tab rename <tab_id> <label>`.
pub async fn tab_rename(herdr_bin: &str, tab_id: &TabId, label: &str) -> Result<()> {
    run(herdr_bin, &["tab", "rename", tab_id.as_str(), label])
        .await
        .map(|_| ())
}

/// `herdr agent wait <pane_id> --status <status> --timeout <timeout_ms>`.
pub async fn agent_wait(
    herdr_bin: &str,
    pane_id: &PaneId,
    status: &str,
    timeout_ms: u64,
) -> Result<()> {
    let timeout_str = timeout_ms.to_string();
    run(
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
    )
    .await
    .map(|_| ())
}

/// `herdr agent send <pane_id> <text>`.
pub async fn agent_send(herdr_bin: &str, pane_id: &PaneId, text: &str) -> Result<()> {
    run(herdr_bin, &["agent", "send", pane_id.as_str(), text])
        .await
        .map(|_| ())
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
}
