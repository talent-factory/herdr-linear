//! Pure decision logic for the "implement this issue" flow triggered by `<Enter>` in an
//! issue list: deriving the preferred coding agent from other open herdr tabs, resolving
//! the final agent command, building the shell-wrapped argv to launch it, building the
//! literal prompt injected once the agent is ready, and picking the right workflow state to
//! move the issue to. No process/socket access here — see [`crate::plugin::herdr_cli`] for
//! that; this module only ever sees JSON text and in-memory values.

use crate::IssueState;
use serde::Deserialize;
use std::collections::HashMap;

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
/// `agent` is null/absent/blank — all of which fall through to [`resolve_agent_command`]'s
/// config/default path.
pub fn resolve_preferred_agent(agent_list_json: &str) -> Option<String> {
    let parsed: AgentListResult = serde_json::from_str(agent_list_json).ok()?;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();

    for entry in &parsed.agents {
        let Some(agent) = entry.agent.as_deref() else {
            continue;
        };
        if agent.trim().is_empty() {
            continue;
        }

        *counts.entry(agent).or_insert(0) += 1;
        if !order.contains(&agent) {
            order.push(agent);
        }
    }

    if order.is_empty() {
        return None;
    }

    // `max_by_key` returns the *last* element among ties, so iterate in reverse to make the
    // *first*-seen agent win ties (see the `_breaks_ties_by_first_seen_order` test below).
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
    fn resolve_preferred_agent_returns_none_when_every_agent_is_blank() {
        let json = r#"{"agents":[{"agent":""},{"agent":"   "}]}"#;

        assert_eq!(resolve_preferred_agent(json), None);
    }

    #[test]
    fn resolve_preferred_agent_returns_none_for_malformed_json() {
        assert_eq!(resolve_preferred_agent("not json"), None);
    }

    #[test]
    fn resolve_agent_command_prefers_the_derived_agent() {
        assert_eq!(resolve_agent_command(Some("claude"), Some("hr")), "claude");
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
        let states = vec![
            state("s1", "Backlog", "backlog"),
            state("s2", "Done", "completed"),
        ];

        assert_eq!(pick_in_progress_state(&states), None);
    }

    #[test]
    fn pick_in_progress_state_returns_none_for_an_empty_list() {
        assert_eq!(pick_in_progress_state(&[]), None);
    }
}
