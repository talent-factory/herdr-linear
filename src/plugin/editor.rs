//! Pure editor-resolution logic for the `c` (open `config.toml`) keybinding — no
//! process/socket access, mirroring `implement.rs`'s charter of keeping decision logic
//! separately unit-testable from the `herdr_cli`/subprocess side that actually acts on it (see
//! `main.rs`'s `open_config_in_herdr_pane`/`open_config_editor`). See
//! docs/superpowers/specs/2026-08-11-editor-handling-design.md for the full three-tier
//! rationale this implements: a `config.toml` `editor` override, else `nvim` if found on
//! `PATH`, else neither (caller falls back to the OS's default opener).

use std::path::{Path, PathBuf};

/// The herdr tab/agent name used for the editor pane `c` opens — deliberately a single, global
/// constant rather than derived from the repo or issue: `config.toml` is itself shared across
/// every repo/workspace this plugin runs in (see `config.rs`'s module doc and README.md's
/// "Configure" section), so every `c` press across every herdr-linear instance should reuse the
/// *same* pane rather than each spawning its own — that's the whole point of
/// `main.rs::open_config_in_herdr_pane` trying `agent_focus` before creating a new tab.
pub const EDITOR_AGENT_NAME: &str = "config";

/// Locates a binary on the system by searching through `PATH` entries.
///
/// Given a `PATH`-like environment variable (colon-separated directories), scans each until
/// `binary` is found as an executable file. Returns `None` if the binary is not found in any
/// directory or if `path_env` is `None`.
///
/// On Windows, also checks for `binary.exe` as a fallback since Windows executables typically
/// have an `.exe` extension.
pub fn find_on_path(path_env: Option<&str>, binary: &str) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env).find_map(|dir| {
        let candidate = dir.join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
        let with_exe = dir.join(format!("{binary}.exe"));
        with_exe.exists().then_some(with_exe)
    })
}

/// Resolves which editor command should be used to open `config.toml`.
///
/// Implements a three-tier priority:
/// 1. If `config_editor` is set (from `config.toml`'s `editor` field), use it.
/// 2. Otherwise, if `nvim` is found on `PATH`, use `"nvim"`.
/// 3. Otherwise, return `None` (caller falls back to OS default opener).
pub fn resolve_editor_command(
    config_editor: Option<String>,
    path_env: Option<&str>,
) -> Option<String> {
    config_editor.or_else(|| find_on_path(path_env, "nvim").map(|_| "nvim".to_string()))
}

/// Constructs the argv list for launching an editor with a config file.
///
/// Given an editor command and the path to the config file, returns a vector with the command
/// and the path as its argument.
pub fn build_editor_argv(editor_cmd: &str, config_path: &Path) -> Vec<String> {
    vec![
        editor_cmd.to_string(),
        config_path.to_string_lossy().into_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_on_path_locates_a_binary_present_in_one_path_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nvim"), "").unwrap();

        let found = find_on_path(Some(dir.path().to_str().unwrap()), "nvim");

        assert_eq!(found, Some(dir.path().join("nvim")));
    }

    #[test]
    fn find_on_path_returns_none_when_binary_is_absent_from_every_entry() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let path_env = format!(
            "{}:{}",
            dir_a.path().to_str().unwrap(),
            dir_b.path().to_str().unwrap()
        );

        let found = find_on_path(Some(&path_env), "nvim");

        assert_eq!(found, None);
    }

    #[test]
    fn find_on_path_skips_nonexistent_entries_and_finds_it_in_a_later_one() {
        let real_dir = tempfile::tempdir().unwrap();
        std::fs::write(real_dir.path().join("nvim"), "").unwrap();
        let path_env = format!(
            "/this/path/does/not/exist:{}",
            real_dir.path().to_str().unwrap()
        );

        let found = find_on_path(Some(&path_env), "nvim");

        assert_eq!(found, Some(real_dir.path().join("nvim")));
    }

    #[test]
    fn find_on_path_returns_none_when_path_env_is_absent() {
        let found = find_on_path(None, "nvim");

        assert_eq!(found, None);
    }

    #[test]
    fn resolve_editor_command_prefers_the_config_override_even_when_nvim_is_on_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nvim"), "").unwrap();

        let resolved = resolve_editor_command(
            Some("emacs".to_string()),
            Some(dir.path().to_str().unwrap()),
        );

        assert_eq!(resolved, Some("emacs".to_string()));
    }

    #[test]
    fn resolve_editor_command_falls_back_to_nvim_when_no_override_and_nvim_is_on_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nvim"), "").unwrap();

        let resolved = resolve_editor_command(None, Some(dir.path().to_str().unwrap()));

        assert_eq!(resolved, Some("nvim".to_string()));
    }

    #[test]
    fn resolve_editor_command_is_none_when_no_override_and_nvim_is_not_on_path() {
        let dir = tempfile::tempdir().unwrap();

        let resolved = resolve_editor_command(None, Some(dir.path().to_str().unwrap()));

        assert_eq!(resolved, None);
    }

    #[test]
    fn build_editor_argv_pairs_the_command_with_the_config_path() {
        let argv = build_editor_argv(
            "nvim",
            Path::new("/home/user/.config/herdr-linear/config.toml"),
        );

        assert_eq!(
            argv,
            vec![
                "nvim".to_string(),
                "/home/user/.config/herdr-linear/config.toml".to_string()
            ]
        );
    }
}
