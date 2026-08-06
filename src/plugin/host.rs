//! Resolves the directory herdr says the user is actually working in.
//!
//! herdr starts every `[[panes]]` pane process from the plugin's own install directory — the
//! manifest's pane `command` (`./target/release/herdr-linear`) is a relative path, so herdr
//! resolves and spawns it against the plugin root, not the workspace the user has focused. A
//! bare `std::env::current_dir()` inside the plugin therefore always resolves to the plugin's
//! own checkout, regardless of which herdr space/tab is active — this is what made both the
//! "Project Issues" view (`repo::detect_repo_name`) and implement-on-Enter
//! (`main.rs`'s `implement_one`, shared by both the single- and multi-issue callers)
//! pick the wrong repo/directory.
//!
//! herdr instead threads the real directory through an injected `HERDR_PLUGIN_CONTEXT_JSON`
//! env var. This is modeled on `herdr-file-viewer`'s `src/host.rs` host adapter — same env var,
//! same field names, same fallback order — since that plugin solved the identical problem (see
//! its `CHANGELOG.md`: "the viewed root follows the *focused herdr pane's* directory"). The two
//! implementations aren't kept in sync automatically; treat this as a starting point, not a
//! guarantee, if `herdr-file-viewer` changes its side later.

use std::path::PathBuf;

/// The shape of `HERDR_PLUGIN_CONTEXT_JSON`. Every field is optional so a partial or absent
/// object degrades gracefully rather than failing to parse; unknown fields are ignored.
#[derive(serde::Deserialize, Default)]
struct RawContext {
    /// herdr 0.7.0+ reports the invoking pane's directory as `focused_pane_cwd` and the
    /// workspace root as `workspace_cwd`; a plain `cwd` is accepted as a fallback.
    focused_pane_cwd: Option<String>,
    workspace_cwd: Option<String>,
    cwd: Option<String>,
}

/// Pick the working directory out of `HERDR_PLUGIN_CONTEXT_JSON`'s parsed payload:
/// `focused_pane_cwd`, then `workspace_cwd`, then a plain `cwd` — the most specific field wins
/// so the plugin roots at the directory the user is actually looking at, not just their
/// workspace. `None` on missing/malformed JSON or when every field is absent or empty, so
/// callers fall back to the process cwd (see [`resolve_cwd`]). Pure function — no I/O — so it's
/// deterministic and safe to unit test.
pub fn parse_context_cwd(json: Option<&str>) -> Option<PathBuf> {
    let raw: RawContext = json
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    raw.focused_pane_cwd
        .filter(|s| !s.trim().is_empty())
        .or_else(|| raw.workspace_cwd.filter(|s| !s.trim().is_empty()))
        .or_else(|| raw.cwd.filter(|s| !s.trim().is_empty()))
        .map(PathBuf::from)
}

/// Resolve the real working directory from the actual environment: `$HERDR_PLUGIN_CONTEXT_JSON`
/// first, falling back to the plugin process's own `std::env::current_dir()` when the env var
/// is absent, malformed, or empty. Thin wrapper around [`parse_context_cwd`]; called from
/// [`crate::plugin::repo::detect_repo_name`] and `main.rs`'s `implement_one` (shared by both
/// the single- and multi-issue "implement this issue" callers). Never fails outright — an
/// unreadable process cwd falls back to an empty [`PathBuf`], modeled on
/// `herdr-file-viewer`'s `host::from_env`. Callers that need the working directory to actually
/// be usable (not just non-panicking) are responsible for checking for that empty case
/// themselves; `implement_one` does.
pub fn resolve_cwd() -> PathBuf {
    let json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok();
    parse_context_cwd(json.as_deref())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_pane_cwd_wins_over_workspace_cwd_and_cwd() {
        let json = r#"{"focused_pane_cwd":"/space/verba","workspace_cwd":"/space","cwd":"/plugin/install"}"#;

        assert_eq!(
            parse_context_cwd(Some(json)),
            Some(PathBuf::from("/space/verba"))
        );
    }

    #[test]
    fn workspace_cwd_wins_when_focused_pane_cwd_is_absent() {
        let json = r#"{"workspace_cwd":"/space","cwd":"/plugin/install"}"#;

        assert_eq!(parse_context_cwd(Some(json)), Some(PathBuf::from("/space")));
    }

    #[test]
    fn plain_cwd_is_the_last_resort_field() {
        let json = r#"{"cwd":"/plugin/install"}"#;

        assert_eq!(
            parse_context_cwd(Some(json)),
            Some(PathBuf::from("/plugin/install"))
        );
    }

    #[test]
    fn empty_string_fields_are_skipped_in_favor_of_the_next_candidate() {
        // A malformed host value (empty string) must not "win" and root at an empty path —
        // fall through to the next candidate instead.
        let json = r#"{"focused_pane_cwd":"","workspace_cwd":"  ","cwd":"/plugin/install"}"#;

        assert_eq!(
            parse_context_cwd(Some(json)),
            Some(PathBuf::from("/plugin/install"))
        );
    }

    #[test]
    fn missing_json_yields_none() {
        assert_eq!(parse_context_cwd(None), None);
    }

    #[test]
    fn malformed_json_yields_none() {
        assert_eq!(parse_context_cwd(Some("not json")), None);
    }

    #[test]
    fn empty_object_yields_none() {
        assert_eq!(parse_context_cwd(Some("{}")), None);
    }

    #[test]
    fn all_fields_blank_yields_none() {
        let json = r#"{"focused_pane_cwd":"","workspace_cwd":"","cwd":""}"#;

        assert_eq!(parse_context_cwd(Some(json)), None);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json =
            r#"{"focused_pane_cwd":"/space/verba","base_branch":"main","workspace_id":"w1"}"#;

        assert_eq!(
            parse_context_cwd(Some(json)),
            Some(PathBuf::from("/space/verba"))
        );
    }
}
