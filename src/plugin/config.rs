//! Resolves the Linear API key, the repo-scoped Linear project override, the
//! `agent_command` override, and the `team_id` default (TF-579) for the plugin: the
//! plugin's own config file first, falling back to environment variables (API key only —
//! there's no environment-variable form of the project, `agent_command`, or `team_id`
//! override).

use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
    /// Repo name (as returned by [`crate::plugin::repo::detect_repo_name`]) → Linear
    /// project id. Looked up case-insensitively (see [`resolve_project_id_override`]) so
    /// a repo whose name doesn't match any Linear project name (see
    /// [`crate::plugin::repo::match_project`]) can still be resolved, *scoped to that one
    /// repo* — unlike the flat `project_id` key this replaces, a single herdr-linear
    /// plugin install shared across many repos/workspaces no longer redirects every one
    /// of them to whichever project was overridden last. `BTreeMap` (not `HashMap`) so
    /// iteration order is deterministic — relevant if this is ever iterated to build a
    /// message (nothing currently does; lookups go through
    /// [`resolve_project_id_override`]'s `.find()` instead). `#[serde(default)]` so a
    /// missing `[project_overrides]` table and an explicitly empty one both deserialize to
    /// the same empty map — every caller already treats "no override for this repo" the
    /// same way regardless of which of those two states produced it, so there's no need
    /// for `Option` to distinguish them.
    #[serde(default)]
    project_overrides: BTreeMap<String, String>,
    agent_command: Option<String>,
    /// Default team id for the Team Issues view (TF-579). Unlike `project_overrides`,
    /// this isn't repo-scoped: Linear teams aren't tied to a single repo the way
    /// projects are, so there's no repo-derived signal to key a table by — see
    /// [`crate::plugin::data::resolve_team_id`] for when this is used (only when the
    /// workspace has more than one team; a single-team workspace never needs it).
    team_id: Option<String>,
    /// The editor command to use when opening config.toml with `c` (TF-614). If set and
    /// non-empty, overrides the default editor resolution — see
    /// [`crate::plugin::editor::resolve_editor_command`]. `None`/unset means default
    /// (`nvim`, `PATH`) resolution.
    editor: Option<String>,
    /// A query DSL string (`crate::plugin::query::parse_query`'s grammar — free text
    /// plus `priority:`/`state:`/`label:`/`sort:` terms), applied automatically every
    /// time a view is entered (TF-617): filter terms narrow the fetch server-side (the
    /// same `IssueFilter` merge TF-616 wired up), `sort:` terms order the result via
    /// `crate::plugin::query::sort_issues`. `None`/unset means no default — a view loads
    /// exactly as it did before this feature. `#[serde(default)]` isn't load-bearing
    /// here the way it is on `project_overrides` above (an `Option<T>` field already
    /// deserializes to `None` when absent) — kept purely for explicitness/documentation
    /// symmetry with that field, not because it changes behavior.
    #[serde(default)]
    default_query: Option<String>,
}

/// Reads and parses `config_dir/config.toml`, if `config_dir` is given and the file
/// exists. `Ok(None)` means there's nothing to read (no config dir, or no file at that
/// path) — that's the normal case, not an error. `Err` when the file exists but isn't
/// valid TOML, or when it exists but couldn't be read (permission denied, not valid UTF-8,
/// a directory in its place, etc.) — only a missing file is silently treated as "no config".
fn read_config_file(config_dir: Option<&Path>) -> Result<Option<ConfigFile>> {
    let Some(dir) = config_dir else {
        return Ok(None);
    };
    let config_path = dir.join("config.toml");
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            tracing::warn!("Failed to read {}: {}", config_path.display(), e);
            return Err(Error::ConfigError(format!(
                "{} exists but could not be read: {}",
                config_path.display(),
                e
            )));
        }
    };
    toml::from_str::<ConfigFile>(&contents)
        .map(Some)
        .map_err(|e| {
            let raw = format!("{} is not valid TOML: {}", config_path.display(), e);
            Error::ConfigError(redact_api_key_value_lines(&raw, &contents))
        })
}

/// The resolved `config.toml` path as a display string, or a placeholder when
/// `config_dir` is unknown (`HERDR_PLUGIN_CONFIG_DIR` unset). Shared by every "nothing
/// resolved" error message in this module and in [`crate::plugin::repo`] so a user always
/// sees exactly which file to edit, never just the bare filename `config.toml`.
pub fn config_path_hint(config_dir: Option<&Path>) -> String {
    config_dir
        .map(|dir| dir.join("config.toml").display().to_string())
        .unwrap_or_else(|| "<HERDR_PLUGIN_CONFIG_DIR not set>/config.toml".to_string())
}

/// Resolve the Linear API key: `config_dir/config.toml`'s `api_key` field first,
/// then `env_api_key`. Pure function — callers own reading the real environment
/// (see [`load`]) so this is deterministic and safe to unit test.
pub fn resolve_api_key(config_dir: Option<&Path>, env_api_key: Option<&str>) -> Result<String> {
    if let Some(file) = read_config_file(config_dir)? {
        if let Some(key) = file.api_key {
            if !key.is_empty() {
                return Ok(key);
            }
        }
        // File parsed fine but no (usable) api_key, fall through to env var.
    }

    if let Some(key) = env_api_key {
        if !key.is_empty() {
            return Ok(key.to_string());
        }
    }

    let path_hint = config_path_hint(config_dir);

    Err(Error::ConfigError(format!(
        "No Linear API key found. Set `api_key` in {path_hint} or export LINEAR_API_KEY."
    )))
}

/// Resolve a `project_id` override for `repo_name`: `config_dir/config.toml`'s
/// `[project_overrides]` table, looked up case-insensitively against `repo_name`
/// (matching [`crate::plugin::repo::match_project`]'s own case-insensitivity), if set and
/// non-empty (returned value is trimmed, same as the key comparison). `Ok(None)` means "no
/// override for this repo" (callers fall back to name matching, see
/// [`crate::plugin::repo::match_project`]) — it is not an error, and it does *not* mean the
/// table is empty: entries for *other* repos are simply ignored. `Err` when two or more
/// `[project_overrides]` keys match `repo_name` case-insensitively (e.g. both `"Repo"` and
/// `"repo"` present) — resolving that silently by picking whichever key happens to sort
/// first would reintroduce the same class of silent misrouting this table exists to
/// prevent, just one level down. Pure function — callers own reading the real environment
/// (see [`load_project_id_override`]).
pub fn resolve_project_id_override(
    config_dir: Option<&Path>,
    repo_name: &str,
) -> Result<Option<String>> {
    let repo_lower = repo_name.trim().to_lowercase();
    let Some(file) = read_config_file(config_dir)? else {
        return Ok(None);
    };

    let matches: Vec<(&String, &String)> = file
        .project_overrides
        .iter()
        .filter(|(key, _)| key.trim().to_lowercase() == repo_lower)
        .collect();

    if matches.len() > 1 {
        let keys = matches
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::ConfigError(format!(
            "{} has {} `[project_overrides]` keys matching repo \"{repo_name}\" \
             case-insensitively: {keys}. Keep only one.",
            config_path_hint(config_dir),
            matches.len(),
        )));
    }

    Ok(matches
        .first()
        .map(|(_, id)| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string))
}

/// Resolve an `agent_command` override: `config_dir/config.toml`'s `agent_command` field, if
/// set and non-empty. `Ok(None)` means "no override" (callers fall back to the agent name
/// derived from other open herdr tabs, then finally `"hr"` — see
/// [`crate::plugin::implement::resolve_agent_command`]). Pure function — callers own reading
/// the real environment (see [`load_agent_command_override`]).
pub fn resolve_agent_command_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let agent_command = read_config_file(config_dir)?
        .and_then(|file| file.agent_command)
        .filter(|cmd| !cmd.trim().is_empty());
    Ok(agent_command)
}

/// Resolve a `default_query` override (TF-617): `config_dir/config.toml`'s
/// `default_query` field, if set and non-empty. `Ok(None)` means "no default query"
/// (callers apply no extra filter/sort beyond whatever a view's own base filter already
/// narrows to — see `crate::plugin::data`'s `fetch_*_issues` functions and `main.rs`'s
/// `load_issues`). Pure function — callers own reading the real environment (see
/// [`load_default_query`]).
pub fn resolve_default_query(config_dir: Option<&Path>) -> Result<Option<String>> {
    let default_query = read_config_file(config_dir)?
        .and_then(|file| file.default_query)
        .filter(|q| !q.trim().is_empty());
    Ok(default_query)
}

/// Resolve a `team_id` default (TF-579): `config_dir/config.toml`'s `team_id` field, if
/// set and non-empty. `Ok(None)` means "no default configured" (callers fall back to
/// resolving the team from the workspace's team list — see
/// `crate::plugin::data::resolve_team_id`), which only errors if that list isn't
/// exactly one team. Pure function — callers own reading the real environment (see
/// [`load_team_id_override`]).
pub fn resolve_team_id_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let team_id = read_config_file(config_dir)?
        .and_then(|file| file.team_id)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    Ok(team_id)
}

/// Resolve an `editor` override: `config_dir/config.toml`'s `editor` field, if set and
/// non-empty. `Ok(None)` means "no override" (callers fall back to the default editor
/// resolution — see [`crate::plugin::editor::resolve_editor_command`]). Pure function —
/// callers own reading the real environment (see [`load_editor_override`]).
pub fn resolve_editor_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let editor = read_config_file(config_dir)?
        .and_then(|file| file.editor)
        .filter(|cmd| !cmd.trim().is_empty());
    Ok(editor)
}

/// Three-way outcome of resolving `config.toml`'s presence/validity — mirrors
/// [`read_config_file`]'s own `Result<Option<ConfigFile>>` shape (`Ok(None)` = missing,
/// `Ok(Some(_))` = parsed, `Err(_)` = present but invalid) rather than collapsing it to a
/// bool, since the help overlay's Settings tab (TF-585) needs to say which of the three
/// it is, not just "found" vs "not".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigFileStatus {
    /// No `config.toml` at the resolved path (or `config_dir` itself unknown).
    NotFound,
    /// `config.toml` exists and parsed successfully.
    Found,
    /// `config.toml` exists but isn't valid TOML (or couldn't be read) — see
    /// [`read_config_file`]'s own doc comment for exactly which cases this covers. The
    /// message is the underlying [`Error`]'s `Display` text, already user-facing — and,
    /// for the invalid-TOML case specifically, already redacted of any `api_key` value by
    /// [`read_config_file`] itself via [`redact_api_key_value_lines`], so this variant
    /// never needs its own redaction pass before display.
    Invalid(String),
}

/// The plugin's currently-effective configuration, for the help overlay's Settings tab
/// (TF-585).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedConfigSummary {
    /// The resolved `config.toml` path, or its `<HERDR_PLUGIN_CONFIG_DIR not set>`
    /// placeholder — see [`config_path_hint`].
    pub path: String,
    pub status: ConfigFileStatus,
    /// True if an API key is currently resolvable from *either* source — mirrors
    /// [`resolve_api_key`]'s own precedence (config file, then `env_api_key`). Never the
    /// raw key value itself, which the Settings tab must not display (TF-585).
    pub api_key_set: bool,
    pub agent_command: Option<String>,
    pub team_id: Option<String>,
    pub editor: Option<String>,
    pub project_overrides: BTreeMap<String, String>,
    /// The resolved `default_query` (TF-617), or `None` if unset/empty.
    pub default_query: Option<String>,
}

/// The `[start, end)` 0-based, end-exclusive line-index range that `contents`'s
/// top-level `api_key` assignment occupies: the line declaring the key itself, through
/// the line just before the next top-level key/table declaration (or end of file) —
/// covering any multi-line (triple-quoted) continuation of its value. `None` if no line
/// declares `api_key` at the top level.
fn api_key_value_line_range(contents: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.iter().position(|line| is_api_key_assignment(line))?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| starts_new_top_level_entry(line))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());
    Some((start, end))
}

/// True if `line` (leading whitespace aside) declares the `api_key` key — matched as the
/// exact, case-sensitive TOML key name [`ConfigFile::api_key`] deserializes from, not a
/// substring match, so a key like `my_api_key_backup` doesn't false-positive.
fn is_api_key_assignment(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("api_key")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// True if `line` looks like the start of a *different* top-level key or table
/// declaration (`some_key = ...` / `[table]`) rather than a continuation of a preceding
/// multi-line value. Blank and comment lines never count as a new entry — either can
/// legitimately appear inside a multi-line string's content, so treating them as a
/// boundary would end the sensitive range early and under-redact.
fn starts_new_top_level_entry(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    if trimmed.starts_with('[') {
        return true;
    }
    let Some((key_part, _)) = trimmed.split_once('=') else {
        return false;
    };
    let key_part = key_part.trim();
    !key_part.is_empty()
        && key_part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Scrubs any line of `message` that echoes a line from `contents`'s `api_key`
/// assignment before the message can reach [`ConfigFileStatus::Invalid`] (final-review
/// fix, TF-585; broadened in a follow-up review after the original single-line-only
/// version — which redacted a message line only if *that line's own text* contained the
/// substring `"api_key"` — was found to still leak a multi-line/triple-quoted `api_key`
/// value: the `toml` crate's parse-error `Display` shows only the *one* source line
/// nearest the error, and for a multi-line string that's typically a continuation line
/// which never contains the literal text `api_key` at all, so a redaction that only
/// inspects the message's own text can never reliably tell that line belongs to the
/// `api_key` field).
///
/// Cross-references against the raw source (`contents`, via
/// [`api_key_value_line_range`]) instead: whichever source lines fall within the
/// `api_key` field's own assignment are treated as sensitive, and any message line that
/// echoes one of them (as a substring, since the `toml` crate's `Display` prefixes each
/// shown line with `"N | "`) is redacted — regardless of whether that message line
/// itself happens to mention `api_key`. Deliberately scoped to this one field (the only
/// secret this file holds), not a general secret scanner.
fn redact_api_key_value_lines(message: &str, contents: &str) -> String {
    let Some((start, end)) = api_key_value_line_range(contents) else {
        // No `api_key` line in the source at all — nothing to redact against.
        return message.to_string();
    };

    let source_lines: Vec<&str> = contents.lines().collect();
    let sensitive: Vec<&str> = source_lines[start..end.min(source_lines.len())]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    message
        .lines()
        .map(|line| {
            if sensitive.iter().any(|snippet| line.contains(snippet)) {
                "[line redacted — contained api_key]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds a [`ResolvedConfigSummary`] for the help overlay's Settings tab (TF-585).
/// Reads `config_dir` exactly once via [`read_config_file`] and derives every field from
/// that single `Result`, rather than composing
/// `resolve_api_key`/`resolve_agent_command_override`/`resolve_team_id_override` (each
/// of which independently re-reads the file and propagates its `Err` via `?`) — that
/// would mean either showing the same "invalid TOML" message three times over, or three
/// separately-worded failures for what is really one root cause. Pure function — callers
/// own reading the real environment, same pattern as every other `resolve_*` function in
/// this module. The `Err` arm's message needs no redaction of its own —
/// [`read_config_file`] already redacts any `api_key` value out of it via
/// [`redact_api_key_value_lines`] before the `Err` is even constructed.
pub(crate) fn resolved_summary(
    config_dir: Option<&Path>,
    env_api_key: Option<&str>,
) -> ResolvedConfigSummary {
    let path = config_path_hint(config_dir);
    let has_env_key = env_api_key.is_some_and(|key| !key.is_empty());

    match read_config_file(config_dir) {
        Ok(None) => ResolvedConfigSummary {
            path,
            status: ConfigFileStatus::NotFound,
            api_key_set: has_env_key,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: BTreeMap::new(),
            default_query: None,
        },
        Ok(Some(file)) => {
            let has_file_key = file.api_key.as_deref().is_some_and(|key| !key.is_empty());
            ResolvedConfigSummary {
                path,
                status: ConfigFileStatus::Found,
                api_key_set: has_file_key || has_env_key,
                agent_command: file.agent_command.filter(|cmd| !cmd.trim().is_empty()),
                team_id: file
                    .team_id
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty()),
                editor: file.editor.filter(|ed| !ed.trim().is_empty()),
                project_overrides: file.project_overrides,
                default_query: file.default_query.filter(|q| !q.trim().is_empty()),
            }
        }
        Err(e) => ResolvedConfigSummary {
            path,
            status: ConfigFileStatus::Invalid(e.to_string()),
            api_key_set: false,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: BTreeMap::new(),
            default_query: None,
        },
    }
}

/// Resolve the Linear API key from the real environment: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`
/// then `$LINEAR_API_KEY`. Thin wrapper around [`resolve_api_key`] used by the binary.
pub fn load() -> Result<String> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    let env_api_key = std::env::var("LINEAR_API_KEY").ok();
    resolve_api_key(config_dir.as_deref(), env_api_key.as_deref())
}

/// Resolve the `project_id` override for `repo_name` from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_project_id_override`]; called from [`crate::plugin::data::fetch_current_project_issues`].
pub fn load_project_id_override(repo_name: &str) -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_project_id_override(config_dir.as_deref(), repo_name)
}

/// Resolve the `agent_command` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_agent_command_override`]; called from `main.rs`'s `implement_one` (shared by both
/// the single- and multi-issue "implement this issue" callers).
pub fn load_agent_command_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_agent_command_override(config_dir.as_deref())
}

/// Resolve the `default_query` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around [`resolve_default_query`];
/// called from `main.rs`'s `load_issues` on every view entry (TF-617).
pub fn load_default_query() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_default_query(config_dir.as_deref())
}

/// Resolve the `team_id` default (TF-579) from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_team_id_override`]; called from
/// `crate::plugin::data::resolve_team_id`.
pub fn load_team_id_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_team_id_override(config_dir.as_deref())
}

/// Resolve the `editor` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_editor_override`]; called from `main.rs`'s OpenConfig handler.
pub fn load_editor_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_editor_override(config_dir.as_deref())
}

/// [`config_path_hint`] resolved against the real environment's `HERDR_PLUGIN_CONFIG_DIR`.
/// Thin wrapper so callers outside this module (e.g.
/// [`crate::plugin::data::fetch_current_project_issues`]'s error-message building, and the
/// `c`-keybinding handler that opens `config.toml`) don't each re-implement the same env
/// var read.
pub fn current_config_path_hint() -> String {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    config_path_hint(config_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::{
        api_key_value_line_range, config_path_hint, redact_api_key_value_lines,
        resolve_agent_command_override, resolve_api_key, resolve_default_query,
        resolve_editor_override, resolve_project_id_override, resolve_team_id_override,
        resolved_summary, ConfigFileStatus,
    };
    use std::fs;

    #[test]
    fn reads_api_key_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_from_file\"\n",
        )
        .unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_file");
    }

    #[test]
    fn falls_back_to_env_var_when_config_file_missing() {
        let dir = tempfile::tempdir().unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_env");
    }

    #[test]
    fn falls_back_to_env_var_when_config_file_has_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "other_field = \"x\"\n").unwrap();

        let key = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap();

        assert_eq!(key, "lin_api_from_env");
    }

    #[test]
    fn errors_with_resolved_path_when_neither_source_has_a_key() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_api_key(Some(dir.path()), None).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("config.toml"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn errors_when_config_dir_is_unknown_and_no_env_var() {
        let err = resolve_api_key(None, None).unwrap_err();

        assert!(err.to_string().contains("LINEAR_API_KEY"));
    }

    #[test]
    fn errors_immediately_on_malformed_toml_without_falling_through_to_env() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
        // Verify it's not just the generic "no key found" message
        assert!(!message.contains("No Linear API key found"));
    }

    #[test]
    fn errors_when_config_file_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in place of config.toml fails to read with something other than
        // NotFound - this must not be silently treated as "no config".
        fs::create_dir(dir.path().join("config.toml")).unwrap();

        let err = resolve_api_key(Some(dir.path()), Some("lin_api_from_env")).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("could not be read"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn config_path_hint_shows_resolved_path_when_dir_known() {
        let dir = tempfile::tempdir().unwrap();

        let hint = config_path_hint(Some(dir.path()));

        assert!(hint.contains(dir.path().to_str().unwrap()));
        assert!(hint.ends_with("config.toml"));
    }

    #[test]
    fn config_path_hint_shows_placeholder_when_dir_unknown() {
        let hint = config_path_hint(None);

        assert_eq!(hint, "<HERDR_PLUGIN_CONFIG_DIR not set>/config.toml");
    }

    #[test]
    fn reads_project_id_override_for_matching_repo_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"examcraft-private\" = \"proj-123\"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, Some("proj-123".to_string()));
    }

    /// The regression this whole redesign exists to prevent: an override entry for one
    /// repo must not leak into the resolution of a *different* repo sharing the same
    /// plugin install / config file.
    #[test]
    fn override_for_one_repo_does_not_apply_to_a_different_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"examcraft-private\" = \"proj-123\"\n",
        )
        .unwrap();

        let project_id = resolve_project_id_override(Some(dir.path()), "herdr-linear").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn multiple_repos_each_resolve_their_own_override() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"repo-a\" = \"proj-a\"\n\"repo-b\" = \"proj-b\"\n",
        )
        .unwrap();

        assert_eq!(
            resolve_project_id_override(Some(dir.path()), "repo-a").unwrap(),
            Some("proj-a".to_string())
        );
        assert_eq!(
            resolve_project_id_override(Some(dir.path()), "repo-b").unwrap(),
            Some("proj-b".to_string())
        );
    }

    #[test]
    fn repo_lookup_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"Examcraft-Private\" = \"proj-123\"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, Some("proj-123".to_string()));
    }

    #[test]
    fn override_value_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"examcraft-private\" = \"  proj-123  \"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, Some("proj-123".to_string()));
    }

    /// Two keys that collide only under case-insensitive comparison must error rather than
    /// silently resolving to whichever one a `BTreeMap` happens to iterate first — that
    /// would reintroduce the same class of silent misrouting this table exists to prevent.
    #[test]
    fn case_colliding_override_keys_error_instead_of_silently_picking_one() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"Repo\" = \"proj-a\"\n\"repo\" = \"proj-b\"\n",
        )
        .unwrap();

        let err = resolve_project_id_override(Some(dir.path()), "repo").unwrap_err();

        let message = err.to_string();
        assert!(message.contains("case-insensitively"));
        assert!(message.contains("Repo"));
        assert!(message.contains("repo"));
    }

    /// A hand-edited config in the middle of migrating from the old flat key to the new
    /// table should resolve the table entries normally, with the stray `project_id` simply
    /// ignored rather than causing a deserialize conflict.
    #[test]
    fn old_flat_key_and_new_table_can_coexist() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "project_id = \"stale-proj\"\n[project_overrides]\n\"examcraft-private\" = \"proj-123\"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, Some("proj-123".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_project_id() {
        let dir = tempfile::tempdir().unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_project_id_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[project_overrides]\n\"examcraft-private\" = \"   \"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_project_overrides() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_project_overrides_table_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "[project_overrides]\n").unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    /// An old-format flat `project_id = "..."` key (pre-redesign) is an unrecognized field
    /// under the new schema — serde silently ignores it rather than erroring, so this
    /// degrades gracefully into "no override found" (surfacing the ordinary no-match error,
    /// which shows the *new* `[project_overrides]` format to adopt) instead of a confusing
    /// TOML parse failure.
    #[test]
    fn old_flat_project_id_key_is_silently_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "project_id = \"proj-123\"\n",
        )
        .unwrap();

        let project_id =
            resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_config_dir_is_unknown_for_project_id() {
        let project_id = resolve_project_id_override(None, "examcraft-private").unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_project_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_project_id_override(Some(dir.path()), "examcraft-private").unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn reads_agent_command_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "agent_command = \"my-agent\"\n",
        )
        .unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, Some("my-agent".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_agent_command() {
        let dir = tempfile::tempdir().unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn returns_none_when_agent_command_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "agent_command = \"   \"\n").unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_agent_command() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let agent_command = resolve_agent_command_override(Some(dir.path())).unwrap();

        assert_eq!(agent_command, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_agent_command() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_agent_command_override(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn reads_default_query_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "default_query = \"priority:>=2 sort:-priority\"\n",
        )
        .unwrap();

        let default_query = resolve_default_query(Some(dir.path())).unwrap();

        assert_eq!(
            default_query,
            Some("priority:>=2 sort:-priority".to_string())
        );
    }

    #[test]
    fn returns_none_when_config_file_missing_for_default_query() {
        let dir = tempfile::tempdir().unwrap();

        let default_query = resolve_default_query(Some(dir.path())).unwrap();

        assert_eq!(default_query, None);
    }

    #[test]
    fn returns_none_when_default_query_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "default_query = \"   \"\n").unwrap();

        let default_query = resolve_default_query(Some(dir.path())).unwrap();

        assert_eq!(default_query, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_default_query() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let default_query = resolve_default_query(Some(dir.path())).unwrap();

        assert_eq!(default_query, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_default_query() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_default_query(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn reads_team_id_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "team_id = \"team-123\"\n").unwrap();

        let team_id = resolve_team_id_override(Some(dir.path())).unwrap();

        assert_eq!(team_id, Some("team-123".to_string()));
    }

    #[test]
    fn team_id_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "team_id = \"  team-123  \"\n",
        )
        .unwrap();

        let team_id = resolve_team_id_override(Some(dir.path())).unwrap();

        assert_eq!(team_id, Some("team-123".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_team_id() {
        let dir = tempfile::tempdir().unwrap();

        let team_id = resolve_team_id_override(Some(dir.path())).unwrap();

        assert_eq!(team_id, None);
    }

    #[test]
    fn returns_none_when_team_id_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "team_id = \"   \"\n").unwrap();

        let team_id = resolve_team_id_override(Some(dir.path())).unwrap();

        assert_eq!(team_id, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_team_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let team_id = resolve_team_id_override(Some(dir.path())).unwrap();

        assert_eq!(team_id, None);
    }

    #[test]
    fn returns_none_when_config_dir_is_unknown_for_team_id() {
        let team_id = resolve_team_id_override(None).unwrap();

        assert_eq!(team_id, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_team_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_team_id_override(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn reads_editor_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "editor = \"vim\"\n").unwrap();

        let editor = resolve_editor_override(Some(dir.path())).unwrap();

        assert_eq!(editor, Some("vim".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_editor() {
        let dir = tempfile::tempdir().unwrap();

        let editor = resolve_editor_override(Some(dir.path())).unwrap();

        assert_eq!(editor, None);
    }

    #[test]
    fn returns_none_when_editor_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "editor = \"   \"\n").unwrap();

        let editor = resolve_editor_override(Some(dir.path())).unwrap();

        assert_eq!(editor, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_editor() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let editor = resolve_editor_override(Some(dir.path())).unwrap();

        assert_eq!(editor, None);
    }

    #[test]
    fn returns_none_when_config_dir_is_unknown_for_editor() {
        let editor = resolve_editor_override(None).unwrap();

        assert_eq!(editor, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_editor() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_editor_override(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
        assert!(message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn resolved_summary_reports_not_found_when_config_dir_is_unknown() {
        let summary = resolved_summary(None, None);

        assert_eq!(summary.status, ConfigFileStatus::NotFound);
        assert!(!summary.api_key_set);
        assert_eq!(summary.agent_command, None);
        assert_eq!(summary.team_id, None);
        assert_eq!(summary.editor, None);
        assert!(summary.project_overrides.is_empty());
    }

    #[test]
    fn resolved_summary_not_found_still_reports_api_key_set_from_env() {
        let dir = tempfile::tempdir().unwrap();

        let summary = resolved_summary(Some(dir.path()), Some("lin_api_from_env"));

        assert_eq!(summary.status, ConfigFileStatus::NotFound);
        assert!(summary.api_key_set);
    }

    #[test]
    fn resolved_summary_reports_found_with_every_resolved_field() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_x\"\nagent_command = \"my-agent\"\nteam_id = \"team-123\"\neditor = \"nvim\"\n\
             default_query = \"priority:>=2\"\n\
             [project_overrides]\n\"herdr-linear\" = \"proj-1\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        assert_eq!(summary.status, ConfigFileStatus::Found);
        assert!(summary.api_key_set);
        assert_eq!(summary.agent_command, Some("my-agent".to_string()));
        assert_eq!(summary.team_id, Some("team-123".to_string()));
        assert_eq!(summary.editor, Some("nvim".to_string()));
        assert_eq!(summary.default_query, Some("priority:>=2".to_string()));
        assert_eq!(
            summary.project_overrides.get("herdr-linear"),
            Some(&"proj-1".to_string())
        );
    }

    #[test]
    fn resolved_summary_found_with_no_file_api_key_still_reports_set_from_env() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "agent_command = \"my-agent\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), Some("lin_api_from_env"));

        assert_eq!(summary.status, ConfigFileStatus::Found);
        assert!(summary.api_key_set);
    }

    #[test]
    fn resolved_summary_found_with_neither_api_key_source_reports_not_set() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "agent_command = \"my-agent\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        assert!(!summary.api_key_set);
    }

    /// Final-review fix (TF-585): an unterminated string on the `api_key` line makes the
    /// `toml` crate's parse-error `Display` embed a snippet of that very line — which
    /// would otherwise put the raw key value on the Settings tab. Confirms
    /// `resolved_summary` redacts it while still surfacing enough of the original error
    /// (e.g. still says the file isn't valid TOML) to be useful.
    #[test]
    fn resolved_summary_redacts_api_key_line_from_invalid_toml_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_SUPERSECRET_XYZ\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        match summary.status {
            ConfigFileStatus::Invalid(message) => {
                assert!(
                    !message.contains("SUPERSECRET"),
                    "raw api_key value leaked into error message: {message}"
                );
                assert!(message.contains("not valid TOML"));
                assert!(message.contains("[line redacted"));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// Follow-up review fix (TF-585): the single-line redaction above only catches the
    /// case where the `toml` crate's error snippet lands *on* the `api_key = ` line
    /// itself. An unterminated multi-line (triple-quoted) `api_key` value makes the
    /// error snippet show a *continuation* line instead — one that never contains the
    /// literal text `api_key` — so the original line-text-matching redaction let the raw
    /// secret straight through. Confirms `resolved_summary` now catches this via
    /// `api_key_value_line_range`'s source-derived range instead of the message text
    /// alone.
    #[test]
    fn resolved_summary_redacts_multiline_api_key_value_from_invalid_toml_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"\"\"\nlin_api_SUPERSECRET_MULTILINE_LEAK\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        match summary.status {
            ConfigFileStatus::Invalid(message) => {
                assert!(
                    !message.contains("SUPERSECRET_MULTILINE_LEAK"),
                    "raw api_key value leaked into error message: {message}"
                );
                assert!(message.contains("not valid TOML"));
                assert!(message.contains("[line redacted"));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// Same leak vector as above, but via TOML's other multi-line string form (a literal
    /// triple-single-quoted string) — confirms the fix isn't accidentally scoped to only
    /// the basic-string case.
    #[test]
    fn resolved_summary_redacts_multiline_literal_api_key_value_from_invalid_toml_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = '''\nlin_api_SUPERSECRET_LITERAL_LEAK\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        match summary.status {
            ConfigFileStatus::Invalid(message) => {
                assert!(
                    !message.contains("SUPERSECRET_LITERAL_LEAK"),
                    "raw api_key value leaked into error message: {message}"
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// Confirms the broadened redaction doesn't over-reach: a syntax error on a
    /// *different, unrelated* field (with a perfectly valid `api_key` earlier in the
    /// file) must still show its real error text — the `toml` crate's snippet here
    /// doesn't even echo the `api_key` line (the error is entirely about
    /// `agent_command`), so nothing should be redacted at all.
    #[test]
    fn resolved_summary_does_not_redact_an_unrelated_field_error_when_api_key_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_totally_fine\"\nagent_command = [this is not valid\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        match summary.status {
            ConfigFileStatus::Invalid(message) => {
                assert!(
                    message.contains("agent_command"),
                    "unrelated field's real error text is missing: {message}"
                );
                assert!(
                    !message.contains("[line redacted"),
                    "an error unrelated to `api_key` was redacted anyway: {message}"
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// Companion to the above: this time the error snippet *does* land on a line that
    /// echoes the (valid) `api_key` value — e.g. a duplicate-key redeclaration — so this
    /// confirms redaction still fires when it should, even though `api_key` itself
    /// parses fine on its own.
    #[test]
    fn resolved_summary_redacts_api_key_value_line_even_when_the_error_is_a_duplicate_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_DUPLICATE_SECRET\"\napi_key = \"lin_api_DUPLICATE_SECRET\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        match summary.status {
            ConfigFileStatus::Invalid(message) => {
                assert!(!message.contains("DUPLICATE_SECRET"));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn api_key_value_line_range_covers_only_the_single_line_when_unterminated() {
        let range = api_key_value_line_range("api_key = \"unterminated\n");
        assert_eq!(range, Some((0, 1)));
    }

    #[test]
    fn api_key_value_line_range_extends_to_end_of_file_for_an_unclosed_multiline_value() {
        let contents = "api_key = \"\"\"\nsecret line one\nsecret line two\n";
        let range = api_key_value_line_range(contents);
        assert_eq!(range, Some((0, 3)));
    }

    #[test]
    fn api_key_value_line_range_stops_at_the_next_top_level_key() {
        let contents = "api_key = \"\"\"\nsecret\n\"\"\"\nagent_command = \"x\"\n";
        let range = api_key_value_line_range(contents);
        assert_eq!(range, Some((0, 3)));
    }

    #[test]
    fn api_key_value_line_range_stops_at_a_table_header() {
        let contents = "api_key = \"\"\"\nsecret\n\"\"\"\n[project_overrides]\n";
        let range = api_key_value_line_range(contents);
        assert_eq!(range, Some((0, 3)));
    }

    #[test]
    fn api_key_value_line_range_is_none_when_api_key_is_absent() {
        assert_eq!(
            api_key_value_line_range("agent_command = \"x\"\nteam_id = \"y\"\n"),
            None
        );
    }

    #[test]
    fn api_key_value_line_range_does_not_match_a_similarly_named_key() {
        // `my_api_key_backup` starts with a different identifier, not the literal
        // `api_key` key — must not be mistaken for it.
        assert_eq!(
            api_key_value_line_range("my_api_key_backup = \"not the real one\"\n"),
            None
        );
    }

    #[test]
    fn redact_api_key_value_lines_passes_message_through_unchanged_when_no_api_key_present() {
        let message = "1 | agent_command = [oops\nnot valid TOML";
        let contents = "agent_command = [oops\n";

        assert_eq!(redact_api_key_value_lines(message, contents), message);
    }

    #[test]
    fn resolved_summary_reports_invalid_on_malformed_toml_with_every_other_field_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let summary = resolved_summary(Some(dir.path()), Some("lin_api_from_env"));

        match summary.status {
            ConfigFileStatus::Invalid(message) => assert!(message.contains("not valid TOML")),
            other => panic!("expected Invalid, got {other:?}"),
        }
        assert!(!summary.api_key_set);
        assert_eq!(summary.agent_command, None);
        assert_eq!(summary.team_id, None);
        assert!(summary.project_overrides.is_empty());
    }
}
