//! Hand-rolled parser for the plugin's query DSL — the structured input consumed by the
//! server-side filter wiring (TF-616) and the config `default_query` / `/`-filter
//! integration (TF-617). No external parser crate: this matches the codebase's existing
//! hand-rolled-parsing style (see [`crate::plugin::config`]'s TOML line handling).
//!
//! ## Grammar
//!
//! This doc comment is the single source of truth for the DSL grammar, in the same spirit
//! as [`crate::plugin::keybindings`]'s keybinding registry doc comment.
//!
//! ```text
//! query        := token (whitespace token)*
//! token        := filter_term | sort_term | free_text
//!
//! filter_term  := "priority:" priority_value
//!               | "state:" NAME
//!               | "label:" NAME
//!
//! priority_value := op? priority_atom
//! op              := "=" | ">=" | "<="        (defaults to "=" when omitted)
//! priority_atom   := DIGIT                     ("0".."4")
//!                  | "urgent" | "high" | "medium" | "low" | "none"
//!
//! sort_term    := "sort:" sort_field ("," sort_field)*
//! sort_field   := "-"? sort_name
//! sort_name    := "priority" | "updated" | "created" | "identifier"
//!
//! free_text    := any token that isn't a recognized filter_term or sort_term
//! ```
//!
//! Unrecognized `key:value` pairs (unknown key, or a value that fails to parse) are *not*
//! hard errors — they fall back to free text. This is deliberate: it keeps the DSL
//! forward-compatible with typos and future keys instead of rejecting the whole query.
//! Free-text tokens are re-joined (space-separated, original order preserved) into
//! [`ParsedQuery::free_text`] for the existing substring matcher
//! ([`crate::plugin::app::matching_issue_indices`], TF-580).

use crate::Issue;
use std::cmp::Ordering;

/// The result of parsing a query DSL string: recognized filter terms, recognized sort
/// keys, and whatever didn't match either — collected back into free text for the
/// existing substring matcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    /// Recognized `priority:`/`state:`/`label:` terms, in the order they appeared.
    pub filters: Vec<FilterTerm>,
    /// Recognized `sort:` fields, in the order they appeared. A single `sort:a,b` token
    /// expands to multiple entries here, still in left-to-right order.
    pub sort_keys: Vec<SortKey>,
    /// Every token that wasn't a recognized filter or sort term, space-joined in their
    /// original relative order.
    pub free_text: String,
}

/// A single recognized filter constraint. Parsing only captures *what* was asked for —
/// applying it against fetched issues (matching `state`/`label` names, wiring `priority`
/// into Linear's `IssueFilter`) is TF-616's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTerm {
    /// `priority:<op><value>` — `value` is always in `0..=4` (Linear's own range: 0 = no
    /// priority, 1 = urgent, 2 = high, 3 = medium, 4 = low).
    Priority { op: PriorityOp, value: i32 },
    /// `state:<name>` — raw, un-normalized name text; matched case-insensitively against
    /// `Issue::state.name` downstream.
    State(String),
    /// `label:<name>` — raw, un-normalized name text; matched against `Issue::labels`
    /// downstream.
    Label(String),
}

/// Comparison operator for a `priority:` filter term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityOp {
    /// `priority:2` or `priority:=2` — exact match.
    Eq,
    /// `priority:>=2` — at least this priority value.
    Ge,
    /// `priority:<=2` — at most this priority value.
    Le,
}

/// One field a `sort:` term asked for, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub field: SortField,
    /// `true` unless the field was prefixed with `-` (e.g. `sort:-priority`).
    pub ascending: bool,
}

/// Fields recognized after a `sort:` key. See the module doc comment's grammar for the
/// full list of valid names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Priority,
    Updated,
    Created,
    Identifier,
}

/// Parses `input` into recognized filter terms, sort keys, and leftover free text. See
/// the module doc comment for the full grammar. Never panics or errors — anything that
/// doesn't parse as a filter or sort term is treated as free text instead.
pub fn parse_query(input: &str) -> ParsedQuery {
    let mut filters = Vec::new();
    let mut sort_keys = Vec::new();
    let mut free_text_tokens: Vec<&str> = Vec::new();

    for token in input.split_whitespace() {
        match token.split_once(':') {
            Some(("priority", value)) => match parse_priority_term(value) {
                Some(term) => filters.push(term),
                None => free_text_tokens.push(token),
            },
            Some(("state", value)) if !value.is_empty() => {
                filters.push(FilterTerm::State(value.to_string()));
            }
            Some(("label", value)) if !value.is_empty() => {
                filters.push(FilterTerm::Label(value.to_string()));
            }
            Some(("sort", value)) if !value.is_empty() => match parse_sort_term(value) {
                Some(keys) => sort_keys.extend(keys),
                None => free_text_tokens.push(token),
            },
            _ => free_text_tokens.push(token),
        }
    }

    ParsedQuery {
        filters,
        sort_keys,
        free_text: free_text_tokens.join(" "),
    }
}

/// Parses a `priority:` term's value (everything after the colon) into a
/// `FilterTerm::Priority`. Returns `None` on anything malformed — an unknown operator
/// prefix, an out-of-range number, or an unrecognized name — so the caller can fall back
/// to treating the whole token as free text.
fn parse_priority_term(value: &str) -> Option<FilterTerm> {
    let (op, rest) = if let Some(rest) = value.strip_prefix(">=") {
        (PriorityOp::Ge, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (PriorityOp::Le, rest)
    } else if let Some(rest) = value.strip_prefix('=') {
        (PriorityOp::Eq, rest)
    } else {
        (PriorityOp::Eq, value)
    };

    let priority = parse_priority_atom(rest)?;
    Some(FilterTerm::Priority {
        op,
        value: priority,
    })
}

/// Parses the value half of a priority filter (after any `=`/`>=`/`<=` prefix has been
/// stripped) into Linear's `0..=4` priority scale.
fn parse_priority_atom(value: &str) -> Option<i32> {
    match value {
        "none" => Some(0),
        "urgent" => Some(1),
        "high" => Some(2),
        "medium" => Some(3),
        "low" => Some(4),
        _ => {
            let parsed: i32 = value.parse().ok()?;
            (0..=4).contains(&parsed).then_some(parsed)
        }
    }
}

/// Parses a `sort:` term's value (everything after the colon, e.g. `-priority,updated`)
/// into an ordered list of [`SortKey`]s. Returns `None` — falling back to free text — if
/// any comma-separated field is empty or unrecognized, rather than silently dropping just
/// that one field and applying the rest.
fn parse_sort_term(value: &str) -> Option<Vec<SortKey>> {
    value.split(',').map(parse_sort_field).collect()
}

fn parse_sort_field(field: &str) -> Option<SortKey> {
    let (ascending, name) = match field.strip_prefix('-') {
        Some(rest) => (false, rest),
        None => (true, field),
    };

    let field = match name {
        "priority" => SortField::Priority,
        "updated" => SortField::Updated,
        "created" => SortField::Created,
        "identifier" => SortField::Identifier,
        _ => return None,
    };

    Some(SortKey { field, ascending })
}

/// Stable-sorts `issues` in place by `sort_keys`, applied in declared order as successive
/// tiebreakers: the first key is primary, each later key only breaks ties left open by
/// the ones before it. An empty `sort_keys` leaves `issues` in its original relative
/// order (stability alone).
///
/// `created_at`/`updated_at` sort on their RFC3339 string representation directly rather
/// than parsing to a datetime type — RFC3339's fixed-width, zero-padded fields make
/// lexicographic string order agree with chronological order, so a parse step would add
/// cost without changing the result.
pub fn sort_issues(issues: &mut [Issue], sort_keys: &[SortKey]) {
    issues.sort_by(|a, b| {
        for key in sort_keys {
            let ordering = match key.field {
                SortField::Priority => a.priority.cmp(&b.priority),
                SortField::Updated => a.updated_at.cmp(&b.updated_at),
                SortField::Created => a.created_at.cmp(&b.created_at),
                SortField::Identifier => a.identifier.cmp(&b.identifier),
            };
            let ordering = if key.ascending {
                ordering
            } else {
                ordering.reverse()
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issue(identifier: &str, priority: i32, created_at: &str, updated_at: &str) -> Issue {
        Issue {
            id: format!("id-{identifier}"),
            identifier: identifier.to_string(),
            title: format!("Issue {identifier}"),
            description: None,
            state: crate::IssueState {
                id: "state-id".to_string(),
                name: "In Progress".to_string(),
                r#type: "started".to_string(),
            },
            priority,
            estimate: None,
            team: crate::Team {
                id: "team-id".to_string(),
                key: "ENG".to_string(),
                name: "Engineering".to_string(),
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            assignee: None,
            creator: None,
            created_at: created_at.to_string(),
            updated_at: updated_at.to_string(),
            started_at: None,
            completed_at: None,
            cycle: None,
            project: None,
            labels: crate::LabelConnection { nodes: Vec::new() },
            url: format!("https://linear.app/team/issue/{identifier}"),
        }
    }

    #[test]
    fn parse_query_of_empty_string_yields_empty_parsed_query() {
        assert_eq!(parse_query(""), ParsedQuery::default());
        assert_eq!(parse_query("   "), ParsedQuery::default());
    }

    #[test]
    fn parse_query_recognizes_all_filter_term_kinds() {
        let parsed = parse_query("priority:>=2 state:In Review label:urgent-fix");
        // "In Review" splits into two whitespace tokens — "Review" has no ':' so it's a
        // bare free-text token, matching the documented "split on whitespace first" rule.
        assert_eq!(
            parsed.filters,
            vec![
                FilterTerm::Priority {
                    op: PriorityOp::Ge,
                    value: 2
                },
                FilterTerm::State("In".to_string()),
                FilterTerm::Label("urgent-fix".to_string()),
            ]
        );
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "Review");
    }

    #[test]
    fn parse_query_priority_accepts_named_levels_and_bare_digit() {
        let parsed = parse_query(
            "priority:urgent priority:high priority:=3 priority:low priority:none priority:4",
        );
        assert_eq!(
            parsed.filters,
            vec![
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 1
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 2
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 3
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 4
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 0
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: 4
                },
            ]
        );
    }

    #[test]
    fn parse_query_is_sort_only_when_input_is_a_single_sort_term() {
        let parsed = parse_query("sort:-priority,updated");
        assert!(parsed.filters.is_empty());
        assert_eq!(
            parsed.sort_keys,
            vec![
                SortKey {
                    field: SortField::Priority,
                    ascending: false
                },
                SortKey {
                    field: SortField::Updated,
                    ascending: true
                },
            ]
        );
        assert_eq!(parsed.free_text, "");
    }

    #[test]
    fn parse_query_handles_mixed_filter_sort_and_free_text_in_original_order() {
        let parsed = parse_query("foo priority:>=2 bar sort:-priority,updated baz");
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: 2
            }]
        );
        assert_eq!(
            parsed.sort_keys,
            vec![
                SortKey {
                    field: SortField::Priority,
                    ascending: false
                },
                SortKey {
                    field: SortField::Updated,
                    ascending: true
                },
            ]
        );
        assert_eq!(parsed.free_text, "foo bar baz");
    }

    #[test]
    fn parse_query_falls_back_malformed_priority_value_to_free_text_instead_of_panicking() {
        let parsed = parse_query("priority:>=9 priority:sideways priority:>= foo");
        assert!(parsed.filters.is_empty());
        assert_eq!(
            parsed.free_text,
            "priority:>=9 priority:sideways priority:>= foo"
        );
    }

    #[test]
    fn parse_query_falls_back_malformed_sort_term_to_free_text() {
        let parsed = parse_query("sort:priority,bogus sort:");
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "sort:priority,bogus sort:");
    }

    #[test]
    fn parse_query_treats_unrecognized_key_value_pairs_as_free_text() {
        let parsed = parse_query("assignee:me typo:oops");
        assert!(parsed.filters.is_empty());
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "assignee:me typo:oops");
    }

    #[test]
    fn parse_query_collects_bare_tokens_as_free_text_joined_back_together() {
        let parsed = parse_query("  fix   the   login   bug  ");
        assert_eq!(parsed.free_text, "fix the login bug");
    }

    #[test]
    fn sort_issues_orders_by_single_key_ascending_and_descending() {
        let mut issues = vec![
            sample_issue("A", 3, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            sample_issue("B", 1, "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
            sample_issue("C", 2, "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
        ];

        sort_issues(
            &mut issues,
            &[SortKey {
                field: SortField::Priority,
                ascending: true,
            }],
        );
        assert_eq!(
            issues
                .iter()
                .map(|i| i.identifier.as_str())
                .collect::<Vec<_>>(),
            vec!["B", "C", "A"]
        );

        sort_issues(
            &mut issues,
            &[SortKey {
                field: SortField::Priority,
                ascending: false,
            }],
        );
        assert_eq!(
            issues
                .iter()
                .map(|i| i.identifier.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "C", "B"]
        );
    }

    #[test]
    fn sort_issues_applies_multiple_keys_in_declared_order_as_tiebreakers() {
        // Same priority for A/B/C; -priority is therefore a no-op tie, and `identifier`
        // breaks the tie ascending. D has a distinct (lower) priority and must stay last
        // once priority descending is honored as the primary key.
        let mut issues = vec![
            sample_issue("C", 2, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            sample_issue("A", 2, "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
            sample_issue("D", 1, "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
            sample_issue("B", 2, "2026-01-04T00:00:00Z", "2026-01-04T00:00:00Z"),
        ];

        sort_issues(
            &mut issues,
            &[
                SortKey {
                    field: SortField::Priority,
                    ascending: false,
                },
                SortKey {
                    field: SortField::Identifier,
                    ascending: true,
                },
            ],
        );

        assert_eq!(
            issues
                .iter()
                .map(|i| i.identifier.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B", "C", "D"]
        );
    }

    #[test]
    fn sort_issues_with_no_sort_keys_preserves_original_order() {
        let mut issues = vec![
            sample_issue("Z", 4, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            sample_issue("Y", 1, "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
        ];

        sort_issues(&mut issues, &[]);

        assert_eq!(
            issues
                .iter()
                .map(|i| i.identifier.as_str())
                .collect::<Vec<_>>(),
            vec!["Z", "Y"]
        );
    }
}
