# CWD → Linear Project Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve the current working directory to a Linear project id — via a `project_id` override in `config.toml`, or by matching a repo name (derived from `git remote`/the cwd) against `LinearClient::get_projects()` results — so the future "Project Issues" view (TF-578) has something to call.

**Architecture:** Two files change. `src/plugin/config.rs` gains a `project_id` override field/resolver, refactored to share file-reading with the existing `api_key` resolver. A new `src/plugin/repo.rs` module holds the repo-name derivation and project-matching logic, split into pure functions (deterministic, unit-tested, no I/O) plus one thin real-environment wrapper — mirroring `config.rs`'s existing `resolve_api_key`/`load` split exactly.

**Tech Stack:** Rust 2021 (rust-version 1.70), `toml` crate (already a dependency behind the `plugin` feature), `tempfile` for filesystem-backed config tests (already a dev-dependency). No new dependencies.

## Global Constraints

- Follow `src/plugin/config.rs`'s existing pattern: a pure resolver function taking already-resolved inputs (no env/fs access itself), plus a thin wrapper that reads the real environment. This is what makes the AC's "kein Netzwerk in den Tests" / "reines, deterministisches Resolving" requirement possible.
- Reuse `Error::ConfigError` for all new error cases (matches how `resolve_api_key` already reports its own "nothing resolved" case) — do not add new `Error` variants.
- No new crate dependencies. `toml` and `tempfile` are already available (behind the `plugin` feature / as a dev-dependency respectively).
- `src/plugin/repo.rs` does not get wired into `main.rs`/`app.rs` in this plan — there is no "Project Issues" view yet to consume it (TF-578, which depends on this task). Only the public API surface is built.
- Every task ends green on `cargo test --all-features` and clean on `cargo clippy --all-targets --all-features -- -D warnings` — both required by this repo's `justfile`'s `check` recipe.

---

### Task 1: `config.rs` — `project_id` override

**Files:**
- Modify: `src/plugin/config.rs` (whole file — see exact current content below)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn resolve_project_id_override(config_dir: Option<&Path>) -> Result<Option<String>>` and `pub fn load_project_id_override() -> Result<Option<String>>` in `crate::plugin::config` — later consumed by `repo::resolve_project_id`'s callers (out of scope for this plan, wired in TF-578).

**Current file content** (for reference — this is what step 3 replaces):

```rust
//! Resolves the Linear API key for the plugin: the plugin's own config file first,
//! falling back to the `LINEAR_API_KEY` environment variable.

use crate::{Error, Result};
use std::path::Path;

#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
}

/// Resolve the Linear API key: `config_dir/config.toml`'s `api_key` field first,
/// then `env_api_key`. Pure function — callers own reading the real environment
/// (see [`load`]) so this is deterministic and safe to unit test.
pub fn resolve_api_key(config_dir: Option<&Path>, env_api_key: Option<&str>) -> Result<String> {
    if let Some(dir) = config_dir {
        let config_path = dir.join("config.toml");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match toml::from_str::<ConfigFile>(&contents) {
                Ok(parsed) => {
                    if let Some(key) = parsed.api_key {
                        if !key.is_empty() {
                            return Ok(key);
                        }
                    }
                    // File parsed fine but no api_key, fall through to env var
                }
                Err(e) => {
                    // File exists but failed to parse - return error immediately
                    return Err(Error::ConfigError(format!(
                        "{} is not valid TOML: {}",
                        config_path.display(),
                        e
                    )));
                }
            }
        }
        // If read_to_string failed, file doesn't exist - fall through to env var
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

/// Resolve the Linear API key from the real environment: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`
/// then `$LINEAR_API_KEY`. Thin wrapper around [`resolve_api_key`] used by the binary.
pub fn load() -> Result<String> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    let env_api_key = std::env::var("LINEAR_API_KEY").ok();
    resolve_api_key(config_dir.as_deref(), env_api_key.as_deref())
}

#[cfg(test)]
mod tests {
    use super::resolve_api_key;
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
}
```

- [ ] **Step 1: Write the failing tests**

Append these 5 tests inside the existing `mod tests` block (right after
`errors_immediately_on_malformed_toml_without_falling_through_to_env`'s closing `}`, still
inside `mod tests { ... }`), and change the test module's import line from
`use super::resolve_api_key;` to `use super::{resolve_api_key, resolve_project_id_override};`:

```rust
    #[test]
    fn reads_project_id_override_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "project_id = \"proj-123\"\n").unwrap();

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features resolve_project_id_override -- --nocapture`
Expected: FAIL to compile — `resolve_project_id_override` is not defined (and the `use`
line now references a name that doesn't exist yet).

- [ ] **Step 3: Implement**

Replace the whole file with:

```rust
//! Resolves the Linear API key and the Linear project override for the plugin: the
//! plugin's own config file first, falling back to environment variables (API key only —
//! there's no environment-variable form of the project override).

use crate::{Error, Result};
use std::path::Path;

#[derive(serde::Deserialize)]
struct ConfigFile {
    api_key: Option<String>,
    project_id: Option<String>,
}

/// Reads and parses `config_dir/config.toml`, if `config_dir` is given and the file
/// exists. `Ok(None)` means there's nothing to read (no config dir, or no file at that
/// path) — that's the normal case, not an error. `Err` only when the file exists but isn't
/// valid TOML.
fn read_config_file(config_dir: Option<&Path>) -> Result<Option<ConfigFile>> {
    let Some(dir) = config_dir else {
        return Ok(None);
    };
    let config_path = dir.join("config.toml");
    let Ok(contents) = std::fs::read_to_string(&config_path) else {
        // File doesn't exist - nothing to read.
        return Ok(None);
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
/// `crate::plugin::repo::resolve_project_id`) — it is not an error. Pure function —
/// callers own reading the real environment (see [`load_project_id_override`]).
pub fn resolve_project_id_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let project_id = read_config_file(config_dir)?
        .and_then(|file| file.project_id)
        .filter(|id| !id.is_empty());
    Ok(project_id)
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
/// [`resolve_project_id_override`] used by the binary.
pub fn load_project_id_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_project_id_override(config_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::{resolve_api_key, resolve_project_id_override};
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
    fn reads_project_id_override_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "project_id = \"proj-123\"\n").unwrap();

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
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::config:: -- --nocapture`
Expected: PASS — all 11 tests in `plugin::config::tests` (6 original + 5 new).

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/config.rs
git commit -m "feat: add project_id override to plugin config (TF-577)

Extends config.toml with an optional project_id field that will let
repo::resolve_project_id short-circuit name matching entirely. Factors
config-file reading out of resolve_api_key into a shared read_config_file
helper so both resolvers share the same parse-and-error-format logic
without behavior change to the existing api_key resolution.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 2: `repo.rs` module + `derive_repo_name`

**Files:**
- Create: `src/plugin/repo.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn derive_repo_name(remote_url: Option<&str>, cwd_dir_name: &str) -> String` in `crate::plugin::repo` — consumed by `detect_repo_name` in Task 5.

**Current `src/plugin/mod.rs` content:**

```rust
//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key resolution), `launch`
//! (open/focus/close/switch decision logic), `app` (TUI state), `ui` (rendering),
//! `data` (Linear data fetching for the plugin).

pub mod app;
pub mod config;
pub mod data;
pub mod launch;
pub mod ui;
```

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/repo.rs` with just the module doc comment and a failing test module (no
implementation yet):

```rust
//! Resolves which Linear project corresponds to the current working directory: derives a
//! repo name from `git remote`/the working directory, then matches it against Linear
//! projects fetched via `LinearClient::get_projects`. A `project_id` override in
//! config.toml (see `crate::plugin::config`) always wins over name matching.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_remote_yields_repo_name() {
        let name = derive_repo_name(
            Some("git@github.com:talent-factory/herdr-linear.git"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn https_remote_yields_repo_name() {
        let name = derive_repo_name(
            Some("https://github.com/talent-factory/herdr-linear.git"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn https_remote_without_git_suffix_yields_repo_name() {
        let name = derive_repo_name(
            Some("https://github.com/talent-factory/herdr-linear"),
            "unused",
        );
        assert_eq!(name, "herdr-linear");
    }

    #[test]
    fn no_remote_falls_back_to_cwd_dir_name() {
        let name = derive_repo_name(None, "my-local-repo");
        assert_eq!(name, "my-local-repo");
    }

    #[test]
    fn empty_remote_falls_back_to_cwd_dir_name() {
        let name = derive_repo_name(Some(""), "my-local-repo");
        assert_eq!(name, "my-local-repo");
    }
}
```

Then register the module in `src/plugin/mod.rs` (needed for the file to be compiled at
all) — replace its content with:

```rust
//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key / project-id resolution),
//! `launch` (open/focus/close/switch decision logic), `app` (TUI state), `ui`
//! (rendering), `data` (Linear data fetching for the plugin), `repo` (CWD → Linear
//! project resolution).

pub mod app;
pub mod config;
pub mod data;
pub mod launch;
pub mod repo;
pub mod ui;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features derive_repo_name -- --nocapture`
Expected: FAIL to compile — `derive_repo_name` is not defined.

- [ ] **Step 3: Implement**

Insert this above the `#[cfg(test)]` block in `src/plugin/repo.rs` (after the module doc
comment):

```rust
/// Derive a repo name to match against Linear project names: parses the last path segment
/// off `remote_url` (handling both `git@host:org/repo.git` SSH and
/// `https://host/org/repo.git` HTTPS forms, stripping a trailing `.git`), falling back to
/// `cwd_dir_name` when no remote URL is available or it doesn't parse to a non-empty name.
/// Pure function; callers own running `git remote get-url origin` and reading the cwd (see
/// [`detect_repo_name`]).
pub fn derive_repo_name(remote_url: Option<&str>, cwd_dir_name: &str) -> String {
    remote_url
        .and_then(parse_repo_name_from_remote)
        .unwrap_or_else(|| cwd_dir_name.to_string())
}

fn parse_repo_name_from_remote(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let last_segment = trimmed.rsplit(['/', ':']).next()?;
    let name = last_segment.strip_suffix(".git").unwrap_or(last_segment);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::repo:: -- --nocapture`
Expected: PASS — all 5 tests in `plugin::repo::tests`.

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/repo.rs src/plugin/mod.rs
git commit -m "feat: add repo module with git-remote-based repo-name derivation (TF-577)

derive_repo_name() parses a repo name out of an SSH or HTTPS git remote
URL, falling back to the cwd's directory name. Pure function — no git
process spawned here, see detect_repo_name (added later in this issue)
for the real-environment wrapper.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: `repo.rs` — `match_project`

**Files:**
- Modify: `src/plugin/repo.rs`

**Interfaces:**
- Consumes: `crate::Project` (existing type, `pub id: String`, `pub name: String`, plus other fields — see `src/models.rs:65-77`).
- Produces: `pub fn match_project<'a>(repo_name: &str, projects: &'a [Project]) -> Result<&'a Project>` in `crate::plugin::repo` — consumed by `resolve_project_id` in Task 4.

- [ ] **Step 1: Write the failing tests**

Add to the top of `src/plugin/repo.rs`, right after the module doc comment:

```rust
use crate::{Error, Project, Result};
```

Add these tests inside `mod tests` (after the existing 5 `derive_repo_name` tests), and add
a `test_project` helper plus a `ProjectStatus` import at the top of `mod tests`:

```rust
    use crate::ProjectStatus;

    fn test_project(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            url: format!("https://linear.app/talent-factory/project/{id}"),
            lead_id: None,
            lead: None,
            status: ProjectStatus {
                id: "status-1".to_string(),
                name: "Started".to_string(),
                r#type: "started".to_string(),
            },
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            start_date: None,
            target_date: None,
        }
    }

    #[test]
    fn exact_match_wins_over_substring_collision() {
        let projects = vec![
            test_project("p1", "herdr-linear"),
            test_project("p2", "herdr-linear-docs"),
        ];

        let matched = match_project("Herdr-Linear", &projects).unwrap();

        assert_eq!(matched.id, "p1");
    }

    #[test]
    fn exact_match_ambiguous_when_multiple_case_variants() {
        let projects = vec![
            test_project("p1", "Herdr-Linear"),
            test_project("p2", "herdr-linear"),
        ];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Multiple Linear projects"));
        assert!(message.contains("herdr-linear"));
    }

    #[test]
    fn substring_match_resolves_when_unique() {
        let projects = vec![test_project("p1", "herdr-linear-plugin")];

        let matched = match_project("herdr-linear", &projects).unwrap();

        assert_eq!(matched.id, "p1");
    }

    #[test]
    fn no_match_errors_and_mentions_project_id_override() {
        let projects = vec![test_project("p1", "totally-unrelated")];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("No Linear project matches"));
        assert!(message.contains("project_id"));
    }

    #[test]
    fn substring_match_ambiguous_with_multiple_candidates() {
        let projects = vec![
            test_project("p1", "herdr-linear-app"),
            test_project("p2", "herdr-linear-docs"),
        ];

        let err = match_project("herdr-linear", &projects).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Multiple Linear projects"));
        assert!(message.contains("herdr-linear-app"));
        assert!(message.contains("herdr-linear-docs"));
    }

    #[test]
    fn no_projects_at_all_errors() {
        let err = match_project("herdr-linear", &[]).unwrap_err();

        assert!(err.to_string().contains("No Linear project matches"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features --lib plugin::repo:: -- --nocapture`
Expected: FAIL to compile — `match_project` is not defined (the `derive_repo_name` tests
from Task 2 still pass once this compiles, but compilation fails first since
`match_project` doesn't exist yet).

- [ ] **Step 3: Implement**

Add below `parse_repo_name_from_remote` in `src/plugin/repo.rs`:

```rust
/// Match `repo_name` against `projects` by name: case-insensitive exact match first — if
/// exactly one project matches, it wins outright even when other projects would also
/// substring-match. Otherwise falls back to a case-insensitive substring match (either
/// direction), which only resolves when it narrows to exactly one project — zero or
/// multiple candidates at either stage are both errors, never a "best guess". Pure
/// function — takes an already-fetched project list, no network access, so it's
/// deterministic and safe to unit test (see [`resolve_project_id`] for the override-aware
/// entry point callers should use).
pub fn match_project<'a>(repo_name: &str, projects: &'a [Project]) -> Result<&'a Project> {
    let repo_lower = repo_name.to_lowercase();

    let exact: Vec<&Project> = projects
        .iter()
        .filter(|p| p.name.to_lowercase() == repo_lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(ambiguous_error(repo_name, &exact));
    }

    let substring: Vec<&Project> = projects
        .iter()
        .filter(|p| {
            let name_lower = p.name.to_lowercase();
            name_lower.contains(&repo_lower) || repo_lower.contains(&name_lower)
        })
        .collect();
    match substring.len() {
        1 => Ok(substring[0]),
        0 => Err(no_match_error(repo_name)),
        _ => Err(ambiguous_error(repo_name, &substring)),
    }
}

fn no_match_error(repo_name: &str) -> Error {
    Error::ConfigError(format!(
        "No Linear project matches repo \"{repo_name}\". Set `project_id` in config.toml to override."
    ))
}

fn ambiguous_error(repo_name: &str, candidates: &[&Project]) -> Error {
    let names = candidates
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Error::ConfigError(format!(
        "Multiple Linear projects match repo \"{repo_name}\": {names}. Set `project_id` in config.toml to disambiguate."
    ))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::repo:: -- --nocapture`
Expected: PASS — all 11 tests in `plugin::repo::tests` (5 from Task 2 + 6 new).

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/repo.rs
git commit -m "feat: add project name matching to repo module (TF-577)

match_project() matches a repo name against fetched Linear projects:
case-insensitive exact match wins outright, else a unique
case-insensitive substring match, else an error naming the project_id
config.toml override as the way to disambiguate. Pure function — takes
an already-fetched project list, no network access in the tests.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 4: `repo.rs` — `resolve_project_id`

**Files:**
- Modify: `src/plugin/repo.rs`

**Interfaces:**
- Consumes: `match_project` (Task 3).
- Produces: `pub fn resolve_project_id(project_id_override: Option<&str>, repo_name: &str, projects: &[Project]) -> Result<String>` in `crate::plugin::repo` — the composition entry point future callers (TF-578) use, combining `config::resolve_project_id_override`'s output with name matching.

- [ ] **Step 1: Write the failing tests**

Add these tests inside `mod tests` (after the `match_project` tests):

```rust
    #[test]
    fn override_short_circuits_without_matching_project() {
        let id = resolve_project_id(Some("proj-999"), "anything", &[]).unwrap();

        assert_eq!(id, "proj-999");
    }

    #[test]
    fn empty_override_falls_back_to_matching() {
        let projects = vec![test_project("p1", "herdr-linear")];

        let id = resolve_project_id(Some(""), "herdr-linear", &projects).unwrap();

        assert_eq!(id, "p1");
    }

    #[test]
    fn no_override_delegates_to_match_project() {
        let projects = vec![test_project("p1", "herdr-linear")];

        let id = resolve_project_id(None, "herdr-linear", &projects).unwrap();

        assert_eq!(id, "p1");
    }

    #[test]
    fn no_override_and_no_match_propagates_error() {
        let err = resolve_project_id(None, "herdr-linear", &[]).unwrap_err();

        assert!(err.to_string().contains("No Linear project matches"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features --lib plugin::repo:: -- --nocapture`
Expected: FAIL to compile — `resolve_project_id` is not defined.

- [ ] **Step 3: Implement**

Add below `ambiguous_error` in `src/plugin/repo.rs`:

```rust
/// Resolve the Linear project id for the current repo: `project_id_override` wins outright
/// when set and non-empty (an empty string is treated as "not set") — `match_project` is
/// never consulted in that case, so `projects` doesn't even need to be populated. Otherwise
/// matches `repo_name` against `projects` via [`match_project`]. Pure function — see
/// [`detect_repo_name`] and `crate::plugin::config::load_project_id_override` for the
/// real-environment entry points callers should compose this with.
pub fn resolve_project_id(
    project_id_override: Option<&str>,
    repo_name: &str,
    projects: &[Project],
) -> Result<String> {
    if let Some(id) = project_id_override {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    match_project(repo_name, projects).map(|p| p.id.clone())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features --lib plugin::repo:: -- --nocapture`
Expected: PASS — all 15 tests in `plugin::repo::tests` (11 from Tasks 2–3 + 4 new).

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/repo.rs
git commit -m "feat: add override-aware project-id resolution to repo module (TF-577)

resolve_project_id() is the composition entry point: a project_id
override wins outright when set, otherwise it delegates to
match_project(). This is the function future callers (the Project
Issues view, TF-578) will use.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 5: `detect_repo_name` real-environment wrapper + final gate

**Files:**
- Modify: `src/plugin/repo.rs`

**Interfaces:**
- Consumes: `derive_repo_name` (Task 2).
- Produces: `pub fn detect_repo_name() -> String` in `crate::plugin::repo` — the real-environment counterpart to `derive_repo_name`, analogous to how `config::load()` wraps `config::resolve_api_key()`.

This function is a thin, impure wrapper (shells out to `git`, reads `std::env::current_dir()`)
and — consistent with `config::load()`, which has no dedicated unit test either — gets no
test of its own; its logic is already covered by `derive_repo_name`'s tests.

- [ ] **Step 1: Implement**

Add at the end of `src/plugin/repo.rs` (after `resolve_project_id`, outside `mod tests`):

```rust
/// Derive the repo name from the real environment: `git remote get-url origin` in the
/// current working directory, falling back to the cwd's directory name. Thin wrapper
/// around [`derive_repo_name`] used by the binary.
pub fn detect_repo_name() -> String {
    let remote_url = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string());

    let cwd_dir_name = std::env::current_dir()
        .ok()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();

    derive_repo_name(remote_url.as_deref(), &cwd_dir_name)
}
```

- [ ] **Step 2: Sanity-check it manually**

Run: `cargo build --all-features && cargo run --features plugin --bin herdr-linear -- --launch-decision < /dev/null`

This doesn't exercise `detect_repo_name` directly (nothing calls it yet — that's TF-578),
but confirms the crate still builds and the existing binary entry points still work with
the new module in place. Separately, sanity-check the function's logic interactively:

Run: `cargo test --all-features --lib plugin:: -- --nocapture 2>&1 | tail -5`
Expected: PASS — test count unchanged from Task 4 (26 total: 11 in `config`, 15 in `repo`),
confirming `detect_repo_name` compiles without breaking anything.

- [ ] **Step 3: Full local gate**

Run the repo's full pre-commit gate:

```bash
just check
```

Expected: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --all-features -- --nocapture` all succeed, ending in `✅ All checks passed!`.

- [ ] **Step 4: Commit**

```bash
git add src/plugin/repo.rs
git commit -m "feat: add detect_repo_name real-environment wrapper (TF-577)

Thin wrapper around derive_repo_name: runs \`git remote get-url origin\`
and reads the cwd for the dirname fallback. Completes the public API
crate::plugin::repo exposes for TF-578 (Project Issues view) to
consume — this task only builds the resolver, nothing calls it yet.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Summary

After all 5 tasks: `src/plugin/config.rs` has a `project_id` override resolver alongside
its existing `api_key` resolver (11 tests total), and a new `src/plugin/repo.rs` module
exposes `derive_repo_name`, `match_project`, `resolve_project_id` (pure, 15 tests total),
plus `detect_repo_name` (real-environment wrapper, untested by design — mirrors
`config::load()`). All acceptance criteria from TF-577 are met:

- ✅ Repo name derived from `git remote`/cwd (`derive_repo_name` + `detect_repo_name`).
- ✅ Matched against `client.get_projects()` results by name (`match_project`, takes a
  `&[Project]` — the caller fetches via `LinearClient::get_projects`).
- ✅ `project_id` override in config.toml, wins when names don't match / are ambiguous
  (`config::resolve_project_id_override` + `repo::resolve_project_id`).
- ✅ Clear error message when no match and no override
  (`no_match_error`/`ambiguous_error`, both mention the `project_id` escape hatch).
- ✅ Unit tests analogous to `config.rs` — pure, deterministic, no network (26 new/changed
  tests total across both files).
