//! Hand-rolled parser for the plugin's query DSL — the structured input consumed by the
//! server-side filter wiring (TF-616) and the config `default_query` / `/`-filter
//! integration (TF-617). No external parser crate: unlike [`crate::plugin::config`],
//! which delegates actual TOML *parsing* to the `toml` crate (its own hand-rolled logic —
//! `redact_api_key_value_lines`/`api_key_value_line_range` — only scans line ranges for
//! secret redaction in error messages, it doesn't parse TOML), this DSL's grammar is
//! small enough (four recognized keys, no nesting) that pulling in a general-purpose
//! query-language crate wouldn't earn its dependency weight.
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
//! NAME         := quoted | bare
//! quoted       := '"' any-char-except-'"'* '"'   (may contain whitespace; an
//!                                                  unterminated quote runs to the end
//!                                                  of the input rather than failing)
//! bare         := non-empty, non-whitespace substring
//!
//! free_text    := any token that isn't a recognized filter_term or sort_term
//! ```
//!
//! All keys (`priority`/`state`/`label`/`sort`), operators, priority level names
//! (`urgent`/`high`/…), and sort field names (`priority`/`updated`/…) are matched
//! case-sensitively, lowercase-only — `Priority:2`, `SORT:priority`, and `priority:HIGH`
//! are all unrecognized and fall back to free text like any other typo.
//!
//! `NAME` may be double-quoted to include whitespace — `state:"In Review"` and
//! `label:"needs design"` tokenize as one token each, quotes stripped. This is the only
//! way to express a multi-word state/label name: `state:In Review` *without* quotes still
//! tokenizes as two whitespace-separated tokens per `query`'s own grammar, so only `In`
//! is recognized as the filter value and `Review` becomes a separate free-text token.
//! Quoting isn't restricted to filter/sort values — a bare quoted phrase in free text,
//! e.g. `"fix login bug"`, is also a single token rather than three.
//!
//! Unrecognized `key:value` pairs (unknown key, or a value that fails to parse) are *not*
//! hard errors — they fall back to free text. This is deliberate: it keeps the DSL
//! forward-compatible with typos and future keys instead of rejecting the whole query.
//! Free-text tokens are re-joined (space-separated, original order preserved) into
//! [`ParsedQuery::free_text`] for the existing substring matcher
//! ([`crate::plugin::app::matching_issue_indices`], TF-580).
//!
//! A token whose *key* is recognized (`priority`/`state`/`label`/`sort`) but whose
//! *value* fails to parse (`priority:>=9`, `sort:bogus`, `state:` with an empty name) is
//! different from a token whose key isn't recognized at all (`assignee:me`): both still
//! fall back to free text the same way, but only the former is also recorded in
//! [`ParsedQuery::rejected`]. That distinction is deliberate — an unrecognized key is
//! meant to be indistinguishable from ordinary free text (so a future key or an unrelated
//! typo doesn't get flagged), while a recognized key with a bad value is exactly the case
//! where a caller might want to tell the user "that filter didn't apply" instead of
//! silently returning a text search that's unlikely to match anything.
//!
//! ### Priority ordering
//!
//! `priority:` and `sort:priority` both operate on Linear's own numeric scale directly —
//! 0 = no priority, 1 = urgent, 2 = high, 3 = medium, 4 = low — not on perceived urgency.
//! `priority:>=2` matches high/medium/low, excluding both no-priority (0) and urgent (1)
//! since they're below 2. `priority:<=2` matches no-priority/urgent/high (0, 1, and 2 are
//! all ≤ 2), excluding medium and low. `sort:priority` ascending therefore orders
//! no-priority first and low last; `sort:-priority` reverses that. This is a deliberate
//! choice, not an oversight: it mirrors the raw value TF-616 will wire straight into
//! Linear's `IssueFilter`, needing no translation there, at the cost of not matching an
//! "urgent-first" intuition.

use crate::Issue;
use std::cmp::Ordering;

/// The result of parsing a query DSL string: recognized filter terms, recognized sort
/// keys, whatever didn't match either — collected back into free text for the existing
/// substring matcher — and which of those leftover tokens were a *recognized* key with a
/// malformed value (as opposed to a key this module has simply never heard of).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ParsedQuery {
    /// Recognized `priority:`/`state:`/`label:` terms, in the order they appeared. A
    /// query may contain more than one term of the same kind (e.g. two `priority:`
    /// terms) — whether repeated terms combine via AND or OR is TF-616's decision to
    /// make when it wires these into Linear's `IssueFilter`, not something this module
    /// picks on the caller's behalf.
    pub filters: Vec<FilterTerm>,
    /// Recognized `sort:` fields, in the order they appeared. A single `sort:a,b` token
    /// expands to multiple entries here, still in left-to-right order.
    pub sort_keys: Vec<SortKey>,
    /// Every token that wasn't a recognized filter or sort term, space-joined in their
    /// original relative order. Includes both genuinely free text and the original text
    /// of every token also present in [`Self::rejected`] — see that field's doc for why
    /// both.
    pub free_text: String,
    /// The original text of every token whose *key* was recognized (`priority`/`state`/
    /// `label`/`sort`) but whose *value* failed to parse, in the order they appeared —
    /// see the module doc's note on the unrecognized-key-vs-malformed-value distinction.
    /// Each entry here is also present in [`Self::free_text`]: this field is additive
    /// information for a caller that wants to surface a hint, not a replacement for the
    /// existing free-text fallback.
    pub rejected: Vec<String>,
}

/// A single recognized filter constraint. Parsing only captures *what* was asked for —
/// applying it against fetched issues (matching `state`/`label` names, wiring `priority`
/// into Linear's `IssueFilter`) is TF-616's job.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterTerm {
    /// `priority:<op><value>` — see [`Priority`] for the range this carries.
    Priority { op: PriorityOp, value: Priority },
    /// `state:<name>` — raw, un-normalized name text, exactly as typed (quotes stripped
    /// if the token was quoted). TF-616 is expected to match this case-insensitively
    /// against `Issue::state.name`; the actual comparison strategy is TF-616's call, not
    /// this module's — normalizing here would just be guessing at it early.
    State(String),
    /// `label:<name>` — raw, un-normalized name text, exactly as typed (quotes stripped
    /// if the token was quoted). TF-616 is expected to match this against `Issue::labels`
    /// downstream; whether that comparison is case-sensitive is also TF-616's call.
    Label(String),
}

/// A priority value, always in Linear's own `0..=4` scale (0 = no priority, 1 = urgent,
/// 2 = high, 3 = medium, 4 = low). The only way to construct one is [`Priority::new`],
/// which range-checks — so a [`FilterTerm::Priority`] can never carry an out-of-range
/// value, even from code outside this module (e.g. a future TF-616 test fixture), unlike
/// a bare `i32` field would allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(i32);

impl Priority {
    /// Range-checked constructor. Returns `None` for anything outside `0..=4`.
    pub fn new(value: i32) -> Option<Self> {
        (0..=4).contains(&value).then_some(Self(value))
    }

    /// The raw `0..=4` value, e.g. for comparison against [`Issue::priority`] or wiring
    /// into Linear's API.
    pub fn value(self) -> i32 {
        self.0
    }
}

/// Comparison operator for a `priority:` filter term. Operates on Linear's raw numeric
/// scale (0 = no priority … 4 = low) directly, not on perceived urgency — see the module
/// doc's "Priority ordering" section for what that means for `>=`/`<=` in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriorityOp {
    /// `priority:2` or `priority:=2` — exact match.
    Eq,
    /// `priority:>=2` — value is at least 2, i.e. high, medium, or low. Excludes both
    /// no-priority (0) and urgent (1), since both are below 2.
    Ge,
    /// `priority:<=2` — value is at most 2, i.e. no-priority, urgent, or high. Excludes
    /// medium (3) and low (4).
    Le,
}

/// One field a `sort:` term asked for, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SortKey {
    pub field: SortField,
    /// `true` unless the field was prefixed with `-` (e.g. `sort:-priority`).
    pub ascending: bool,
}

/// Fields recognized after a `sort:` key. See the module doc comment's grammar for the
/// full list of valid names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortField {
    Priority,
    Updated,
    Created,
    Identifier,
}

/// Parses `input` into recognized filter terms, sort keys, and leftover free text. See
/// the module doc comment for the full grammar. Never panics or errors — anything that
/// doesn't parse as a filter or sort term is treated as free text instead, with a
/// recognized-but-malformed term additionally recorded in [`ParsedQuery::rejected`] (see
/// that field's doc for the unrecognized-key distinction).
pub fn parse_query(input: &str) -> ParsedQuery {
    let mut filters = Vec::new();
    let mut sort_keys = Vec::new();
    let mut free_text_tokens: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();

    for token in tokenize(input) {
        match token.split_once(':') {
            Some(("priority", value)) => match parse_priority_term(value) {
                Some(term) => filters.push(term),
                None => reject(&mut rejected, &mut free_text_tokens, token),
            },
            Some(("state", value)) => {
                if value.is_empty() {
                    reject(&mut rejected, &mut free_text_tokens, token);
                } else {
                    filters.push(FilterTerm::State(value.to_string()));
                }
            }
            Some(("label", value)) => {
                if value.is_empty() {
                    reject(&mut rejected, &mut free_text_tokens, token);
                } else {
                    filters.push(FilterTerm::Label(value.to_string()));
                }
            }
            Some(("sort", value)) => {
                if value.is_empty() {
                    reject(&mut rejected, &mut free_text_tokens, token);
                } else {
                    match parse_sort_term(value) {
                        Some(keys) => sort_keys.extend(keys),
                        None => reject(&mut rejected, &mut free_text_tokens, token),
                    }
                }
            }
            _ => free_text_tokens.push(token),
        }
    }

    ParsedQuery {
        filters,
        sort_keys,
        free_text: free_text_tokens.join(" "),
        rejected,
    }
}

/// Records `token` as both rejected (a recognized key with a malformed value) and, per
/// this module's forward-compatible fallback policy, still-usable free text.
fn reject(rejected: &mut Vec<String>, free_text_tokens: &mut Vec<String>, token: String) {
    rejected.push(token.clone());
    free_text_tokens.push(token);
}

/// Splits `input` into tokens on whitespace, honoring double-quoted spans as a single
/// token even when they contain whitespace (`state:"In Review"` tokenizes as one token,
/// not two) — see the module doc's `NAME` grammar production. Quotes are stripped from
/// the resulting token; an unterminated quote (no closing `"`) runs to the end of the
/// input rather than being treated as an error, consistent with this module never
/// erroring on malformed input.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
        } else if c == '"' {
            in_token = true;
            for quoted in chars.by_ref() {
                if quoted == '"' {
                    break;
                }
                current.push(quoted);
            }
        } else {
            in_token = true;
            current.push(c);
        }
    }

    if in_token {
        tokens.push(current);
    }
    tokens
}

/// Parses a `priority:` term's value (everything after the colon) into a
/// `FilterTerm::Priority`. Returns `None` on anything malformed — an unknown operator
/// prefix, an out-of-range number, or an unrecognized name — so the caller can fall back
/// to treating the whole token as free text (and record it as rejected).
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
/// stripped) into Linear's `0..=4` priority scale. The bare-digit form accepts exactly
/// one ASCII digit character, matching the grammar's `DIGIT` production directly —
/// `04`, `+2`, `-0`, and multi-digit values like `10` are all rejected outright rather
/// than being accepted and only then range-checked, so what's accepted matches what's
/// documented instead of depending on however permissive `str::parse::<i32>` happens to
/// be.
fn parse_priority_atom(value: &str) -> Option<Priority> {
    match value {
        "none" => Priority::new(0),
        "urgent" => Priority::new(1),
        "high" => Priority::new(2),
        "medium" => Priority::new(3),
        "low" => Priority::new(4),
        _ => {
            let mut chars = value.chars();
            let digit = chars.next()?;
            if chars.next().is_some() {
                return None; // more than one character — not a bare DIGIT
            }
            Priority::new(digit.to_digit(10)? as i32)
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
/// than parsing to a datetime type. This is correct only because Linear's API always
/// returns these fields with a `Z` (UTC) suffix and fixed-width, zero-padded fields — not
/// because lexicographic and chronological order agree for RFC3339 strings in general
/// (they don't across differing timezone offsets: `"...T10:00:00+05:00"` sorts *after*
/// `"...T08:00:00Z"` lexicographically despite being chronologically earlier, since
/// `1` > `0` as characters). If Linear ever emitted a non-`Z` offset for these fields,
/// this comparison would need revisiting.
///
/// `SortField::Priority` sorts on `Issue::priority`'s raw `0..=4` value directly — see
/// the module doc's "Priority ordering" section for what ascending/descending mean under
/// Linear's lower-is-more-urgent scale.
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

    fn priority(value: i32) -> Priority {
        Priority::new(value).unwrap()
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
        // Multi-word names require quoting (see parse_query_quoted_state_and_label_...).
        assert_eq!(
            parsed.filters,
            vec![
                FilterTerm::Priority {
                    op: PriorityOp::Ge,
                    value: priority(2)
                },
                FilterTerm::State("In".to_string()),
                FilterTerm::Label("urgent-fix".to_string()),
            ]
        );
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "Review");
        assert!(parsed.rejected.is_empty());
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
                    value: priority(1)
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(2)
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(3)
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(4)
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(0)
                },
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(4)
                },
            ]
        );
    }

    #[test]
    fn parse_query_priority_le_operator_is_recognized() {
        let parsed = parse_query("priority:<=2");
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::Priority {
                op: PriorityOp::Le,
                value: priority(2)
            }]
        );
    }

    #[test]
    fn parse_query_priority_bare_digit_must_be_a_single_in_range_ascii_digit() {
        let parsed =
            parse_query("priority:04 priority:+2 priority:-0 priority:10 priority:5 priority:4");
        // Only the trailing single-digit, in-range token parses; every other malformed
        // variant — leading zero, sign prefix, multi-digit, and single-digit-but-out-of-
        // range — falls back and is recorded as rejected.
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::Priority {
                op: PriorityOp::Eq,
                value: priority(4)
            }]
        );
        assert_eq!(
            parsed.rejected,
            vec![
                "priority:04",
                "priority:+2",
                "priority:-0",
                "priority:10",
                "priority:5"
            ]
        );
    }

    #[test]
    fn priority_new_accepts_only_0_through_4() {
        assert_eq!(Priority::new(0).map(Priority::value), Some(0));
        assert_eq!(Priority::new(4).map(Priority::value), Some(4));
        assert_eq!(Priority::new(-1), None);
        assert_eq!(Priority::new(5), None);
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
    fn parse_query_accumulates_multiple_separate_sort_tokens_in_order() {
        let parsed = parse_query("sort:priority sort:-updated");
        assert_eq!(
            parsed.sort_keys,
            vec![
                SortKey {
                    field: SortField::Priority,
                    ascending: true
                },
                SortKey {
                    field: SortField::Updated,
                    ascending: false
                },
            ]
        );
    }

    #[test]
    fn parse_query_handles_mixed_filter_sort_and_free_text_in_original_order() {
        let parsed = parse_query("foo priority:>=2 bar sort:-priority,updated baz");
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::Priority {
                op: PriorityOp::Ge,
                value: priority(2)
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
        // "foo" isn't a recognized key at all, so it's genuinely free text rather than
        // rejected — only the three malformed `priority:` tokens land in `rejected`.
        assert_eq!(
            parsed.rejected,
            vec!["priority:>=9", "priority:sideways", "priority:>="]
        );
    }

    #[test]
    fn parse_query_falls_back_malformed_sort_term_to_free_text() {
        let parsed = parse_query("sort:priority,bogus sort:");
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "sort:priority,bogus sort:");
        assert_eq!(parsed.rejected, vec!["sort:priority,bogus", "sort:"]);
    }

    #[test]
    fn parse_query_empty_state_and_label_values_are_rejected_and_fall_back_to_free_text() {
        let parsed = parse_query("state: label:");
        assert!(parsed.filters.is_empty());
        assert_eq!(parsed.rejected, vec!["state:", "label:"]);
        assert_eq!(parsed.free_text, "state: label:");
    }

    #[test]
    fn parse_query_treats_unrecognized_key_value_pairs_as_free_text() {
        let parsed = parse_query("assignee:me typo:oops");
        assert!(parsed.filters.is_empty());
        assert!(parsed.sort_keys.is_empty());
        assert_eq!(parsed.free_text, "assignee:me typo:oops");
        // Unrecognized keys are deliberately *not* rejected — see the module doc's note
        // on why that's different from a recognized key with a malformed value.
        assert!(parsed.rejected.is_empty());
    }

    #[test]
    fn parse_query_rejected_only_includes_recognized_keys_with_malformed_values() {
        let parsed = parse_query("priority:oops assignee:me sort:nope typo:oops");
        assert_eq!(parsed.rejected, vec!["priority:oops", "sort:nope"]);
        assert_eq!(
            parsed.free_text,
            "priority:oops assignee:me sort:nope typo:oops"
        );
    }

    #[test]
    fn parse_query_collects_bare_tokens_as_free_text_joined_back_together() {
        let parsed = parse_query("  fix   the   login   bug  ");
        assert_eq!(parsed.free_text, "fix the login bug");
    }

    #[test]
    fn parse_query_tokenizes_on_tabs_and_newlines_too() {
        let parsed = parse_query("priority:2\tstate:X\nlabel:Y");
        assert_eq!(
            parsed.filters,
            vec![
                FilterTerm::Priority {
                    op: PriorityOp::Eq,
                    value: priority(2)
                },
                FilterTerm::State("X".to_string()),
                FilterTerm::Label("Y".to_string()),
            ]
        );
    }

    #[test]
    fn parse_query_state_value_may_contain_additional_colons() {
        // Only the *first* colon splits key from value — this is a deliberate
        // consequence of `split_once`, not something the grammar restricts further.
        let parsed = parse_query("state:foo:bar");
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::State("foo:bar".to_string())]
        );
    }

    #[test]
    fn parse_query_quoted_state_and_label_values_preserve_internal_whitespace() {
        let parsed = parse_query(r#"state:"In Review" label:"needs design""#);
        assert_eq!(
            parsed.filters,
            vec![
                FilterTerm::State("In Review".to_string()),
                FilterTerm::Label("needs design".to_string()),
            ]
        );
        assert_eq!(parsed.free_text, "");
    }

    #[test]
    fn parse_query_quoted_free_text_phrase_stays_a_single_token() {
        let parsed = parse_query(r#""fix login bug" urgent"#);
        assert_eq!(parsed.free_text, "fix login bug urgent");
    }

    #[test]
    fn parse_query_unterminated_quote_runs_to_end_of_input_without_panicking() {
        let parsed = parse_query(r#"state:"In Review"#);
        assert_eq!(
            parsed.filters,
            vec![FilterTerm::State("In Review".to_string())]
        );
    }

    #[test]
    fn parse_query_empty_quoted_value_is_rejected_like_any_other_empty_name() {
        let parsed = parse_query(r#"state:"""#);
        assert!(parsed.filters.is_empty());
        assert_eq!(parsed.rejected, vec!["state:"]);
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
    fn sort_issues_by_priority_ascending_orders_no_priority_before_urgent() {
        // Pins the raw-value (not urgency) semantics documented in the module doc's
        // "Priority ordering" section: 0 (no priority) sorts before 1 (urgent).
        let mut issues = vec![
            sample_issue("U", 1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            sample_issue("N", 0, "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
            sample_issue("L", 4, "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z"),
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
            vec!["N", "U", "L"]
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
    fn sort_issues_orders_by_created_and_updated_timestamps_lexicographically() {
        let mut issues = vec![
            sample_issue("A", 2, "2026-03-01T00:00:00Z", "2026-01-05T00:00:00Z"),
            sample_issue("B", 2, "2026-01-01T00:00:00Z", "2026-03-05T00:00:00Z"),
            sample_issue("C", 2, "2026-02-01T00:00:00Z", "2026-02-05T00:00:00Z"),
        ];

        sort_issues(
            &mut issues,
            &[SortKey {
                field: SortField::Created,
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
                field: SortField::Updated,
                ascending: true,
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
    fn sort_issues_preserves_relative_order_for_issues_tied_on_every_provided_key() {
        // Both issues share the same priority — the one provided key ties completely, so
        // the comparator must fall through to `Ordering::Equal` and stability alone
        // decides the outcome, rather than any key's tiebreaker logic ever firing.
        let mut issues = vec![
            sample_issue("First", 2, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            sample_issue("Second", 2, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
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
            vec!["First", "Second"]
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
