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
        .ok_or_else(|| Error::Internal("agent.start response missing agent.pane_id".to_string()))?
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
pub async fn agent_wait(
    herdr_bin: &str,
    pane_id: &str,
    status: &str,
    timeout_ms: u64,
) -> Result<()> {
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
