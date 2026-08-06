//! Resolves the Linear API key, the Linear project override, and the `agent_command` override
//! for the plugin: the plugin's own config file first, falling back to environment variables
//! (API key only — there's no environment-variable form of the project or `agent_command`
//! override).

use crate::{Error, Result};
use std::path::Path;

#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
    project_id: Option<String>,
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

    let path_hint = config_dir
        .map(|dir| dir.join("config.toml").display().to_string())
        .unwrap_or_else(|| "<HERDR_PLUGIN_CONFIG_DIR not set>/config.toml".to_string());

    Err(Error::ConfigError(format!(
        "No Linear API key found. Set `api_key` in {path_hint} or export LINEAR_API_KEY."
    )))
}

/// Resolve a `project_id` override: `config_dir/config.toml`'s `project_id` field, if set
/// and non-empty. `Ok(None)` means "no override" (callers fall back to name matching, see
/// [`crate::plugin::repo::resolve_project_id`]) — it is not an error. Pure function —
/// callers own reading the real environment (see [`load_project_id_override`]).
pub fn resolve_project_id_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let project_id = read_config_file(config_dir)?
        .and_then(|file| file.project_id)
        .filter(|id| !id.trim().is_empty());
    Ok(project_id)
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

/// Resolve the `project_id` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_project_id_override`]; called from [`crate::plugin::data::fetch_current_project_issues`].
pub fn load_project_id_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_project_id_override(config_dir.as_deref())
}

/// Resolve the `agent_command` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around
/// [`resolve_agent_command_override`]; called from `main.rs`'s `start_implementation`.
pub fn load_agent_command_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_agent_command_override(config_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::{resolve_agent_command_override, resolve_api_key, resolve_project_id_override};
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
    fn reads_project_id_override_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "project_id = \"proj-123\"\n",
        )
        .unwrap();

        let project_id = resolve_project_id_override(Some(dir.path())).unwrap();

        assert_eq!(project_id, Some("proj-123".to_string()));
    }

    #[test]
    fn returns_none_when_config_file_missing_for_project_id() {
        let dir = tempfile::tempdir().unwrap();

        let project_id = resolve_project_id_override(Some(dir.path())).unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_project_id_is_empty_or_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "project_id = \"   \"\n").unwrap();

        let project_id = resolve_project_id_override(Some(dir.path())).unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_config_file_has_no_project_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "api_key = \"lin_api_x\"\n").unwrap();

        let project_id = resolve_project_id_override(Some(dir.path())).unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn returns_none_when_config_dir_is_unknown_for_project_id() {
        let project_id = resolve_project_id_override(None).unwrap();

        assert_eq!(project_id, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_project_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "this is [invalid toml\n").unwrap();

        let err = resolve_project_id_override(Some(dir.path())).unwrap_err();

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
