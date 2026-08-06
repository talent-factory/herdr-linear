//! Resolves the Linear API key, the repo-scoped Linear project override, and the
//! `agent_command` override for the plugin: the plugin's own config file first, falling
//! back to environment variables (API key only — there's no environment-variable form of
//! the project or `agent_command` override).

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
            Error::ConfigError(format!(
                "{} is not valid TOML: {}",
                config_path.display(),
                e
            ))
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
        config_path_hint, resolve_agent_command_override, resolve_api_key,
        resolve_project_id_override,
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
}
