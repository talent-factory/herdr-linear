# EDITOR handling for `c` (open config.toml) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single OS-default-opener call behind `c` (open `config.toml`) with a
three-tier resolution: `nvim` in a herdr pane (default) → a configured `editor` override (same
herdr-pane mechanism) → today's `open::that` OS opener (fallback), so `c` is actually usable over
SSH (e.g. herdr on an iPad) where there is no GUI.

**Architecture:** Two new/extended modules (`src/plugin/editor.rs` for pure resolution logic,
`herdr_cli::agent_focus` for a thin subprocess wrapper) plus new orchestration functions in
`src/main.rs` (`open_config_in_herdr_pane`, `open_config_editor`) that replace the body of the
existing `Action::OpenConfig` handler. No new `Action` variant — `handle_key`/`app.rs` (PR #31)
are untouched.

**Tech Stack:** Rust, tokio (async), the `herdr` CLI's JSON socket protocol (via
`plugin::herdr_cli`'s existing subprocess wrapper), `toml`/`serde` for config parsing, `open`
crate for the OS-opener fallback. No new dependencies.

## Global Constraints

- Design source of truth: `docs/superpowers/specs/2026-08-11-editor-handling-design.md` — read
  it before starting; this plan implements it task-by-task.
- No new crate dependencies (PATH lookup uses `std::env::split_paths`, not a `which` crate).
- The `editor` config value is a bare binary name only — no shell-string parsing, no flags.
- `EDITOR_AGENT_NAME` is `"config"` — a single, deliberately global (not per-repo) herdr
  tab/agent name, since `config.toml` is itself shared across every repo/workspace this plugin
  runs in.
- Silent fallback: no status message is shown when *any* tier succeeds — only when the final
  (`open::that`) tier also fails, using the exact same message format as today:
  `"Couldn't open {path}: {e}. Edit it manually."`.
- First success wins: once a tier succeeds, later tiers (including `open::that`) must never also
  run — the file must never be opened twice.
- Every new async orchestration function takes `herdr_bin: &str` as an explicit parameter (never
  calls `plugin::herdr_cli::herdr_bin()` internally) so it stays testable against a fake `herdr`
  script — mirrors the existing `implement_one`/`resolve_validated_agent_command` convention,
  where only the outer, real-environment-reading call site (`start_implementation`) is left
  untested.

---

## File Structure

- **`src/plugin/config.rs`** (modified) — `editor` field on `ConfigFile`, `resolve_editor_override`,
  `load_editor_override`, `editor` field on `ResolvedConfigSummary`.
- **`src/plugin/ui.rs`** (modified) — Settings tab (`settings_lines_from`) renders the new
  `editor` field; its 4 existing test fixtures gain the field.
- **`src/plugin/editor.rs`** (new) — pure decision logic: `find_on_path`, `resolve_editor_command`,
  `EDITOR_AGENT_NAME` constant, `build_editor_argv`. No process/socket access — mirrors
  `implement.rs`'s charter.
- **`src/plugin/mod.rs`** (modified) — registers the new `editor` module.
- **`src/plugin/herdr_cli.rs`** (modified) — new `agent_focus` wrapper.
- **`src/main.rs`** (modified) — `open_config_in_herdr_pane`, `open_config_editor`,
  `resolve_editor_command_from_env`, the `Action::OpenConfig` handler rewritten to use them, and
  `CONFIG_TEMPLATE` gains a commented `# editor = "vim"` line.
- **`README.md`** (modified) — "Configure" section documents the `editor` key and corrects the
  now-inaccurate "opens this file with your OS's default handler" sentence.
- **`CHANGELOG.md`** (modified) — `[Unreleased] > Added` entry.

---

### Task 1: `editor` override in `config.rs`

**Files:**
- Modify: `src/plugin/config.rs`
- Modify: `src/plugin/ui.rs` (the Settings tab renders every `ResolvedConfigSummary` field by
  name — adding `editor` to the struct without also handling it here would leave 4 existing test
  fixtures with a missing-field compile error, since they construct
  `ResolvedConfigSummary { .. }` as full struct literals, not `..Default::default()`)

**Interfaces:**
- Produces: `pub fn resolve_editor_override(config_dir: Option<&Path>) -> Result<Option<String>>`,
  `pub fn load_editor_override() -> Result<Option<String>>`, and a new `pub editor: Option<String>`
  field on `pub(crate) struct ResolvedConfigSummary`. Task 4 calls `load_editor_override()`.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/config.rs`'s `mod tests` block, right after the existing
`returns_none_when_config_file_missing_for_agent_command` test (around line 735):

```rust
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
    fn returns_none_when_config_dir_is_unknown_for_editor() {
        let editor = resolve_editor_override(None).unwrap();

        assert_eq!(editor, None);
    }

    #[test]
    fn errors_immediately_on_malformed_toml_for_editor() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "not valid toml{{{").unwrap();

        let err = resolve_editor_override(Some(dir.path())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("not valid TOML"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features editor -- --nocapture`
Expected: FAIL with `cannot find function \`resolve_editor_override\` in this scope`.

- [ ] **Step 3: Add the `editor` field and `resolve_editor_override`**

In `src/plugin/config.rs`, add `editor: Option<String>` to `struct ConfigFile` right after the
`agent_command` field (around line 30):

```rust
    agent_command: Option<String>,
    /// Editor command used to open `config.toml` when `c` is pressed, launched inside a herdr
    /// pane — see [`crate::plugin::editor::resolve_editor_command`]. A bare binary name only
    /// (no shell parsing, no flags), consistent with how it's passed straight through as a
    /// single `argv` element rather than interpreted by a shell. `None`/unset means the default
    /// (`nvim`, if found on `PATH`) applies instead. See
    /// docs/superpowers/specs/2026-08-11-editor-handling-design.md.
    editor: Option<String>,
```

Then add `resolve_editor_override`, right after `resolve_agent_command_override` (around line 163):

```rust
/// Resolve an `editor` override: `config_dir/config.toml`'s `editor` field, if set and
/// non-empty. `Ok(None)` means "no override" — callers fall back to `nvim` if it's on `PATH`,
/// then finally the OS's default opener. See
/// [`crate::plugin::editor::resolve_editor_command`]. Pure function — callers own reading the
/// real environment (see [`load_editor_override`]).
pub fn resolve_editor_override(config_dir: Option<&Path>) -> Result<Option<String>> {
    let editor = read_config_file(config_dir)?
        .and_then(|file| file.editor)
        .filter(|cmd| !cmd.trim().is_empty());
    Ok(editor)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features editor -- --nocapture`
Expected: PASS (5 tests).

- [ ] **Step 5: Add `editor` to `ResolvedConfigSummary` and `load_editor_override`**

In `src/plugin/config.rs`, add the field to `ResolvedConfigSummary` right after `agent_command`
(around line 211):

```rust
    pub agent_command: Option<String>,
    pub editor: Option<String>,
    pub team_id: Option<String>,
```

Wire it into all three arms of `resolved_summary` (around lines 327–357) — add
`editor: None,` to the `Ok(None)` and `Err(e)` arms, and to the `Ok(Some(file))` arm add:

```rust
                editor: file.editor.filter(|cmd| !cmd.trim().is_empty()),
```

placed right after the `agent_command:` line in that arm.

Then add `load_editor_override`, right after `load_agent_command_override` (around line 383):

```rust
/// Resolve the `editor` override from the real environment:
/// `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. Thin wrapper around [`resolve_editor_override`];
/// called from `main.rs`'s `resolve_editor_command_from_env`.
pub fn load_editor_override() -> Result<Option<String>> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_editor_override(config_dir.as_deref())
}
```

- [ ] **Step 6: Update the existing `resolved_summary` field-coverage tests**

In `resolved_summary_reports_found_with_every_resolved_field` (around line 863), add `editor` to
the written config and assert it:

```rust
    #[test]
    fn resolved_summary_reports_found_with_every_resolved_field() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "api_key = \"lin_api_x\"\nagent_command = \"my-agent\"\nteam_id = \"team-123\"\n\
             editor = \"vim\"\n\
             [project_overrides]\n\"herdr-linear\" = \"proj-1\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        assert_eq!(summary.status, ConfigFileStatus::Found);
        assert!(summary.api_key_set);
        assert_eq!(summary.agent_command, Some("my-agent".to_string()));
        assert_eq!(summary.editor, Some("vim".to_string()));
        assert_eq!(summary.team_id, Some("team-123".to_string()));
        assert_eq!(
            summary.project_overrides.get("herdr-linear"),
            Some(&"proj-1".to_string())
        );
    }
```

In `resolved_summary_reports_not_found_when_config_dir_is_unknown` (around line 842), add:

```rust
        assert_eq!(summary.editor, None);
```

right after the existing `assert_eq!(summary.agent_command, None);` line.

- [ ] **Step 7: Surface `editor` in the Settings tab (`src/plugin/ui.rs`)**

`ResolvedConfigSummary` now has 6 fields, but `settings_lines_from`'s 4 test fixtures (around
lines 2179, 2202, 2224, 2246) construct it as a full struct literal — they'll fail to compile
with a missing-field error the moment Step 5 lands, whether or not the new field is ever
displayed. Fix that *and* actually surface the field (an unset/misconfigured `editor` is exactly
the kind of thing this diagnostic tab exists to show).

First, add the failing assertion to the existing "found" test (around line 2220) — insert
`editor: Some("vim".to_string()),` into that test's struct literal right after
`agent_command: Some("my-agent".to_string()),`, and add a new assertion:

```rust
        assert!(lines.contains("vim"));
```

right after the existing `assert!(lines.contains("my-agent"));` line.

Run: `cargo test --all-features settings_lines -- --nocapture`
Expected: FAIL to compile — `missing field \`editor\` in initializer of \`ResolvedConfigSummary\`` —
confirming the other 3 fixtures need the field too before this even reaches a runtime assertion.

Add `editor: None,` to the other 3 struct literals (the `NotFound`/`Invalid`-status fixtures
around lines 2179, 2202, 2246), each right after their `agent_command: None,` line.

Run: `cargo test --all-features settings_lines -- --nocapture`
Expected: compiles now; FAILS at the new `assert!(lines.contains("vim"));` (the "found" test) —
`settings_lines_from` doesn't render `editor` yet.

Then add the rendering line in `settings_lines_from` (`src/plugin/ui.rs`, right after the
`agent_command` line, around line 869):

```rust
    let editor_display = summary.editor.as_deref().unwrap_or("(default: nvim if on PATH)");
    lines.push(format!("editor           = {editor_display}"));
```

(11 spaces between `editor` and `=` — aligns the `=` column with `api_key`/`agent_command`/
`team_id` above it, each of which pads to the same 17-character column width.)

Run: `cargo test --all-features settings_lines -- --nocapture`
Expected: PASS (all 4 tests).

- [ ] **Step 8: Run the full `config.rs` and `ui.rs` test suites**

Run: `cargo test --all-features 2>&1 | tail -30`
Expected: PASS, all tests (existing + new, no regressions in either file).

- [ ] **Step 9: Commit**

```bash
git add src/plugin/config.rs src/plugin/ui.rs
git commit -m "feat: resolve an \`editor\` override from config.toml, surface it in Settings (TF-614)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 2: `src/plugin/editor.rs` — pure editor resolution

**Files:**
- Create: `src/plugin/editor.rs`
- Modify: `src/plugin/mod.rs`

**Interfaces:**
- Consumes: nothing from other tasks (pure, no `config.rs`/`herdr_cli.rs` calls — callers pass
  already-resolved values in).
- Produces: `pub fn find_on_path(path_env: Option<&str>, binary: &str) -> Option<PathBuf>`,
  `pub fn resolve_editor_command(config_editor: Option<String>, path_env: Option<&str>) -> Option<String>`,
  `pub const EDITOR_AGENT_NAME: &str = "config"`,
  `pub fn build_editor_argv(editor_cmd: &str, config_path: &Path) -> Vec<String>`. Task 3 and
  Task 4 use all four.

- [ ] **Step 1: Write the failing tests**

Create `src/plugin/editor.rs` with just the test module first:

```rust
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
        let argv = build_editor_argv("nvim", Path::new("/home/user/.config/herdr-linear/config.toml"));

        assert_eq!(
            argv,
            vec![
                "nvim".to_string(),
                "/home/user/.config/herdr-linear/config.toml".to_string()
            ]
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --all-features plugin::editor:: -- --nocapture`
Expected: FAIL with `cannot find function \`find_on_path\`` (and the other three) `in this scope`.

- [ ] **Step 3: Implement `find_on_path`, `resolve_editor_command`, `build_editor_argv`**

Add above the `#[cfg(test)]` block in `src/plugin/editor.rs`:

```rust
/// Scans `path_env` (a `PATH`-style, platform-separated list of directories — injected rather
/// than read directly from `std::env::var` so this stays pure and testable, same pattern as
/// every `resolve_*` function in `config.rs`) for `binary`, returning the first match's full
/// path. Existence-only check (`Path::exists`), not an executable-bit check — good enough to
/// answer "would running this command find something", the same bar `open::that`'s own
/// OS-level lookup effectively applies. `binary.exe` is also tried per directory so this works
/// unmodified on Windows, mirroring how `implement_one` already resolves the user's shell via
/// `$SHELL` on Unix only elsewhere in this codebase — no Windows-specific shell handling exists
/// today, but a plain existence check here costs nothing extra to make cross-platform.
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

/// Resolves which editor command `c` should open `config.toml` with, per the three-tier
/// priority in docs/superpowers/specs/2026-08-11-editor-handling-design.md: `config_editor`
/// (already read from `config.toml` by the caller — see
/// `crate::plugin::config::resolve_editor_override`/`load_editor_override`) wins if set,
/// regardless of whether `nvim` is also on `PATH`; otherwise `"nvim"` if [`find_on_path`]
/// locates it; otherwise `None` — the caller falls back to the OS's default opener
/// (`open::that`) in that case. Pure — both inputs are already resolved by the caller, so this
/// makes no I/O of its own.
pub fn resolve_editor_command(
    config_editor: Option<String>,
    path_env: Option<&str>,
) -> Option<String> {
    config_editor.or_else(|| find_on_path(path_env, "nvim").map(|_| "nvim".to_string()))
}

/// Builds the `argv` passed to `herdr agent start -- <argv...>` for the editor pane: the
/// resolved editor command followed by the config file's path. A single-element command with a
/// single path argument — no shell interpretation, matching the "bare binary name, no flags"
/// contract `config.toml`'s `editor` key documents.
pub fn build_editor_argv(editor_cmd: &str, config_path: &Path) -> Vec<String> {
    vec![editor_cmd.to_string(), config_path.display().to_string()]
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --all-features plugin::editor:: -- --nocapture`
Expected: PASS (8 tests).

- [ ] **Step 5: Register the module**

In `src/plugin/mod.rs`, add `pub mod editor;` alphabetically (between `data` and `herdr_cli`),
and extend the module-doc list at the top:

```rust
//! Support modules for the herdr-linear plugin binary.
//!
//! Submodules are added incrementally: `config` (API key / project-id resolution),
//! `launch` (open/focus/close/switch decision logic), `app` (TUI state), `ui`
//! (rendering), `data` (Linear data fetching for the plugin), `editor` (pure editor-resolution
//! logic for the `c` keybinding), `repo` (CWD → Linear project resolution), `herdr_cli` (herdr
//! CLI subprocess wrapper), `implement` (pure decision logic for "implement this issue" flow),
//! `host` (resolves the herdr-injected launch context's working directory, since the plugin
//! process's own cwd is always its install directory), `keybindings` (canonical keybindings
//! registry for the help overlay).

pub mod app;
pub mod config;
pub mod data;
pub mod editor;
pub mod herdr_cli;
pub mod host;
pub mod implement;
pub mod keybindings;
pub mod launch;
pub mod repo;
pub mod ui;
```

- [ ] **Step 6: Run the full test suite and clippy**

Run: `cargo test --all-features 2>&1 | tail -30`
Expected: PASS, all tests (no regressions).

Run: `cargo clippy --all-features --all-targets 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/plugin/editor.rs src/plugin/mod.rs
git commit -m "feat: add plugin::editor with pure nvim/config-override resolution (TF-614)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 3: `herdr_cli::agent_focus`

**Files:**
- Modify: `src/plugin/herdr_cli.rs`

**Interfaces:**
- Produces: `pub async fn agent_focus(herdr_bin: &str, target: &str) -> Result<()>`. Task 4's
  `open_config_in_herdr_pane` calls it.

No dedicated test for the subprocess-spawning wrapper itself — this module's own doc comment
already documents that convention ("The subprocess-spawning half is deliberately untested at
this layer... The response-interpretation half (`interpret_output`) is pure and unit-tested
below", which `agent_focus` reuses unchanged via `run`). Task 4's fake-`herdr`-script tests
exercise `agent_focus` indirectly, the same way `implement_one`'s tests exercise `tab_create`/
`agent_start`/`pane_close` indirectly rather than each having its own subprocess-level test.

- [ ] **Step 1: Add `agent_focus`**

In `src/plugin/herdr_cli.rs`, add right after `pane_close` (around line 524):

```rust
/// `herdr agent focus <target>`. Used by `main.rs`'s `open_config_in_herdr_pane` to reuse an
/// already-open editor pane instead of creating a duplicate: `target` accepts a unique agent
/// name (per herdr's own target resolution), so passing [`crate::plugin::editor::EDITOR_AGENT_NAME`]
/// here finds the pane a previous `c` press already created via [`agent_start`], if any. Any
/// error — most commonly `agent_not_found` (verified live against herdr 0.7.3: fails
/// immediately with `{"error":{"code":"agent_not_found",...}}`, no timeout wait) — is treated
/// identically by the caller: "not there, create it". Unlike [`agent_start`], there is no
/// special-casing of any particular error code here, since there's only one thing to do next
/// regardless of *why* focus failed.
pub async fn agent_focus(herdr_bin: &str, target: &str) -> Result<()> {
    run(herdr_bin, &["agent", "focus", target])
        .await
        .map(|_| ())
}
```

- [ ] **Step 2: Run the full test suite and clippy**

Run: `cargo test --all-features 2>&1 | tail -10`
Expected: PASS (no new tests added, no regressions — confirms the crate still compiles and
nothing else broke).

Run: `cargo clippy --all-features --all-targets 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 3: Commit**

```bash
git add src/plugin/herdr_cli.rs
git commit -m "feat: add herdr_cli::agent_focus for reusing an existing editor pane (TF-614)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 4: `main.rs` orchestration and `OpenConfig` handler rewrite

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `plugin::editor::{EDITOR_AGENT_NAME, build_editor_argv, resolve_editor_command}`,
  `plugin::config::load_editor_override`, `plugin::herdr_cli::{agent_focus, tab_create,
  agent_start, pane_close, herdr_bin}`, `plugin::host::resolve_cwd`.
- Produces: `async fn open_config_in_herdr_pane(herdr_bin: &str, editor_cmd: &str, config_path: &Path) -> std::result::Result<(), String>`,
  `async fn open_config_editor(path: &Path, editor_cmd: Option<String>, herdr_bin: &str, opener: impl Fn(&Path) -> std::io::Result<()>) -> std::result::Result<(), String>`,
  `fn resolve_editor_command_from_env() -> Option<String>`. Nothing outside `main.rs` consumes
  these (they're private `fn`s, same visibility as `implement_one`).

- [ ] **Step 1: Add the test-only fake-`herdr`-script helper for editor tests**

In `src/main.rs`'s `mod tests` block, add right after `write_fake_herdr_script` (around line 988,
just before `resolve_validated_agent_command_resolves_from_agent_list_without_a_config_override`):

```rust
    /// Fake `herdr` script dispatching `agent focus`, `tab create`, `agent start`, and
    /// `pane close` calls to canned bodies — the four subcommands
    /// `open_config_in_herdr_pane` can issue. A shorter, purpose-built sibling of
    /// `write_dispatching_herdr_script` (which also covers `agent wait`, irrelevant here: the
    /// editor flow never waits on agent status).
    fn write_editor_herdr_script(
        agent_focus: &str,
        tab_create: &str,
        agent_start: &str,
        pane_close: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        write_fake_herdr_script(&format!(
            r#"
case "$1 $2" in
  "agent focus") {agent_focus} ;;
  "tab create") {tab_create} ;;
  "agent start") {agent_start} ;;
  "pane close") {pane_close} ;;
  *)
    echo '{{"error":{{"message":"unexpected herdr call: $1 $2"}}}}'
    exit 1
    ;;
esac
"#
        ))
    }
```

- [ ] **Step 2: Write the failing tests for `open_config_in_herdr_pane`**

Add right after `write_editor_herdr_script`:

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_focuses_an_existing_pane_without_creating_a_tab() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"result":{}}'; exit 0"#,
            r#"echo 'tab create should not run'; exit 1"#,
            r#"echo 'agent start should not run'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_creates_a_tab_when_no_pane_exists_yet() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"code":"agent_not_found","message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t2"}}}'; exit 0"#,
            r#"echo '{"result":{}}'; exit 0"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_succeeds_even_when_cleanup_pane_close_fails() {
        // The editor already opened successfully in `started`'s pane by the time `pane_close`
        // runs — a leftover empty pane is cosmetic, not a reason to report failure (which would
        // make the caller fall back to `open::that` and open the file a second time).
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"result":{"agent":{"pane_id":"p1","tab_id":"t2"}}}'; exit 0"#,
            r#"echo '{"error":{"message":"no such pane"}}'; exit 1"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_does_not_close_a_pane_when_agent_replaced_the_root_pane() {
        // `agent_start`'s pane id equals the tab's root pane id here (herdr replaced rather
        // than split) — `pane close` must not run at all; the script fails loudly if it does.
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"result":{"agent":{"pane_id":"p9","tab_id":"t2"}}}'; exit 0"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        assert_eq!(result, Ok(()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_fails_when_tab_create_fails_after_a_focus_miss() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
            r#"echo 'agent start should not run'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("failed to create a tab") && message.contains("no such workspace"),
            "unexpected message: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_in_herdr_pane_fails_when_agent_start_fails_after_a_focus_miss() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"result":{"tab":{"tab_id":"t2","label":"config"},"root_pane":{"pane_id":"p9"}}}'; exit 0"#,
            r#"echo '{"error":{"message":"no such tab"}}'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );

        let result = open_config_in_herdr_pane(
            script.to_str().unwrap(),
            "nvim",
            std::path::Path::new("/fake/config/dir/config.toml"),
        )
        .await;

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("failed to start") && message.contains("no such tab"),
            "unexpected message: {message}"
        );
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --all-features open_config_in_herdr_pane -- --nocapture`
Expected: FAIL with `cannot find function \`open_config_in_herdr_pane\` in this scope` (6 tests).

- [ ] **Step 4: Implement `open_config_in_herdr_pane`**

In `src/main.rs`, add right after `implement_one` ends (its closing `}` is around line 627),
before `start_implementation`'s doc comment ("/// Single-issue `<Enter>` flow...", around line
629) — so the two herdr-pane-opening flows (`implement_one` and this one) sit next to each
other:

```rust
/// Runs `editor_cmd` on `config_path` inside a herdr pane, for the `c` keybinding: reuses an
/// already-open editor pane if a previous `c` press created one ([`plugin::herdr_cli::agent_focus`]
/// on [`plugin::editor::EDITOR_AGENT_NAME`]), otherwise opens a fresh tab for it
/// ([`plugin::herdr_cli::tab_create`] + [`plugin::herdr_cli::agent_start`], closing the tab's
/// now-redundant root pane exactly like [`implement_one`] does). The pane/tab/agent name is
/// always [`plugin::editor::EDITOR_AGENT_NAME`] — deliberately global, not derived from repo or
/// issue; see that constant's own doc for why a single shared pane across every herdr-linear
/// instance is correct here, unlike `implement_one`'s per-issue names. A cleanup `pane_close`
/// failure is logged but never turned into an `Err` — the editor itself already opened
/// successfully in `started`'s pane by that point, and reporting failure here would make the
/// caller fall back to the OS opener and open the file a second time. See
/// docs/superpowers/specs/2026-08-11-editor-handling-design.md.
async fn open_config_in_herdr_pane(
    herdr_bin: &str,
    editor_cmd: &str,
    config_path: &std::path::Path,
) -> std::result::Result<(), String> {
    if plugin::herdr_cli::agent_focus(herdr_bin, plugin::editor::EDITOR_AGENT_NAME)
        .await
        .is_ok()
    {
        return Ok(());
    }

    let cwd = config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(plugin::host::resolve_cwd);
    let argv = plugin::editor::build_editor_argv(editor_cmd, config_path);

    let created_tab = plugin::herdr_cli::tab_create(herdr_bin, &cwd, plugin::editor::EDITOR_AGENT_NAME)
        .await
        .map_err(|err| format!("failed to create a tab: {err}"))?;

    let started = plugin::herdr_cli::agent_start(
        herdr_bin,
        plugin::editor::EDITOR_AGENT_NAME,
        &cwd,
        &created_tab.tab_id,
        &argv,
    )
    .await
    .map_err(|err| format!("tab created but the editor failed to start: {err}"))?;

    if started.pane_id != created_tab.root_pane_id {
        if let Err(err) = plugin::herdr_cli::pane_close(herdr_bin, &created_tab.root_pane_id).await
        {
            tracing::warn!("failed to close the config tab's redundant empty pane: {err}");
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --all-features open_config_in_herdr_pane -- --nocapture`
Expected: PASS (6 tests).

- [ ] **Step 6: Write the failing tests for `open_config_editor`**

Add right after the `open_config_in_herdr_pane` tests:

```rust
    #[tokio::test]
    async fn open_config_editor_calls_the_opener_when_no_editor_resolved() {
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            None,
            "/nonexistent/herdr-should-not-run",
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        )
        .await;

        assert_eq!(result, Ok(()));
        assert_eq!(
            opener_calls.into_inner(),
            vec![std::path::PathBuf::from("/fake/config/dir/config.toml")]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_editor_does_not_call_the_opener_when_the_herdr_pane_succeeds() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"result":{}}'; exit 0"#,
            r#"echo 'tab create should not run'; exit 1"#,
            r#"echo 'agent start should not run'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            script.to_str().unwrap(),
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(
            opener_calls.into_inner().is_empty(),
            "opener must not run when the herdr-pane path already succeeded"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_editor_falls_back_to_the_opener_when_the_herdr_pane_path_fails() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
            r#"echo 'agent start should not run'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );
        let opener_calls = std::cell::RefCell::new(Vec::new());

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            script.to_str().unwrap(),
            |p| {
                opener_calls.borrow_mut().push(p.to_path_buf());
                Ok(())
            },
        )
        .await;

        assert_eq!(result, Ok(()));
        assert_eq!(
            opener_calls.into_inner(),
            vec![std::path::PathBuf::from("/fake/config/dir/config.toml")]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_config_editor_fails_when_both_the_herdr_pane_and_the_opener_fail() {
        let (_dir, script) = write_editor_herdr_script(
            r#"echo '{"error":{"message":"agent target config not found"}}'; exit 1"#,
            r#"echo '{"error":{"message":"no such workspace"}}'; exit 1"#,
            r#"echo 'agent start should not run'; exit 1"#,
            r#"echo 'pane close should not run'; exit 1"#,
        );

        let result = open_config_editor(
            std::path::Path::new("/fake/config/dir/config.toml"),
            Some("nvim".to_string()),
            script.to_str().unwrap(),
            |_p| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no handler registered",
                ))
            },
        )
        .await;

        let Err(message) = result else {
            panic!("expected Err, got {result:?}");
        };
        assert!(
            message.contains("Couldn't open") && message.contains("no handler registered"),
            "unexpected message: {message}"
        );
    }
```

- [ ] **Step 7: Run tests to verify they fail**

Run: `cargo test --all-features open_config_editor -- --nocapture`
Expected: FAIL with `cannot find function \`open_config_editor\` in this scope` (4 tests).

- [ ] **Step 8: Implement `open_config_editor` and `resolve_editor_command_from_env`**

Add right after `open_config_in_herdr_pane`:

```rust
/// Resolves which editor `c` should use from the real environment: `config.toml`'s `editor`
/// override (via [`plugin::config::load_editor_override`]), else `nvim` if on `$PATH` (via
/// [`plugin::editor::resolve_editor_command`]), else `None`. A malformed `config.toml` degrades
/// to "no override" rather than failing outright — the same resilience `resolved_summary`
/// already applies to every optional field on invalid TOML — since an unrelated pre-existing
/// config error shouldn't block `c` from opening *some* editor. Not unit-tested itself (a thin
/// real-environment-reading wrapper, same status as `herdr_cli::herdr_bin`/`config::load`) —
/// [`plugin::config::resolve_editor_override`] and [`plugin::editor::resolve_editor_command`]
/// each already cover the decision logic this composes.
fn resolve_editor_command_from_env() -> Option<String> {
    let config_editor = plugin::config::load_editor_override().unwrap_or_else(|err| {
        tracing::warn!("couldn't read editor override from config.toml: {err}");
        None
    });
    plugin::editor::resolve_editor_command(config_editor, std::env::var("PATH").ok().as_deref())
}

/// Opens `config.toml` for the `c` keybinding: if `editor_cmd` resolved to something (see
/// [`resolve_editor_command_from_env`]), tries [`open_config_in_herdr_pane`] first; on success,
/// `opener` is never called — the file must never be opened twice. Otherwise (`editor_cmd` is
/// `None`, or the herdr-pane attempt failed) calls `opener(path)` — `open::that` in production,
/// today's unchanged OS-default-opener fallback. `herdr_bin` and `opener` are both explicit
/// parameters (rather than resolved internally via `plugin::herdr_cli::herdr_bin()`/`open::that`
/// directly) so this whole function stays testable against a fake `herdr` script and a fake
/// opener — mirrors how `implement_one` takes `herdr_bin: &str` while only its real-environment
/// caller (`start_implementation`) is left untested. See
/// docs/superpowers/specs/2026-08-11-editor-handling-design.md.
async fn open_config_editor(
    path: &std::path::Path,
    editor_cmd: Option<String>,
    herdr_bin: &str,
    opener: impl Fn(&std::path::Path) -> std::io::Result<()>,
) -> std::result::Result<(), String> {
    if let Some(cmd) = &editor_cmd {
        if open_config_in_herdr_pane(herdr_bin, cmd, path).await.is_ok() {
            return Ok(());
        }
    }

    opener(path).map_err(|e| format!("Couldn't open {}: {e}", path.display()))
}
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test --all-features open_config_editor -- --nocapture`
Expected: PASS (4 tests).

- [ ] **Step 10: Rewrite the `Action::OpenConfig` event_loop handler**

In `src/main.rs`'s `event_loop`, replace the entire `plugin::app::Action::OpenConfig(path) => { ... }`
arm (the block starting with the "Unlike `OpenInBrowser` above..." comment) with:

```rust
                        plugin::app::Action::OpenConfig(path) => {
                            // Unlike `OpenInBrowser` above, this chains filesystem writes and
                            // (possibly) a herdr round-trip in front of the final "open it"
                            // step — each with real, user-hittable failure modes (permission
                            // denied, disk full, herdr unreachable) — and it's the sole recovery
                            // action offered on the error screen. Silently doing nothing here
                            // would leave the user stuck with no indication that pressing `c`
                            // didn't work, so unlike `OpenInBrowser` this surfaces a failure via
                            // `set_status` rather than discarding it. On success, no status is
                            // shown at all regardless of which tier provided it (deliberate —
                            // see docs/superpowers/specs/2026-08-11-editor-handling-design.md's
                            // "silent fallback").
                            let ensure_result: Result<(), String> = (|| {
                                if let Some(parent) = path.parent() {
                                    std::fs::create_dir_all(parent).map_err(|e| {
                                        format!("Couldn't create {}: {e}", parent.display())
                                    })?;
                                }
                                if !path.exists() {
                                    std::fs::write(&path, CONFIG_TEMPLATE).map_err(|e| {
                                        format!("Couldn't write {}: {e}", path.display())
                                    })?;
                                }
                                Ok(())
                            })();

                            match ensure_result {
                                Err(message) => {
                                    app.set_status(plugin::app::Status::Error(format!(
                                        "{message}. Edit it manually."
                                    )));
                                }
                                Ok(()) => {
                                    let editor_cmd = resolve_editor_command_from_env();
                                    // Only shown when a herdr round-trip is actually about to
                                    // happen — the OS-opener-only path (`editor_cmd` is `None`)
                                    // is normally near-instant, so a "loading" status for it
                                    // would just flicker.
                                    if editor_cmd.is_some() {
                                        app.set_status(plugin::app::Status::Ok(
                                            "Opening config.toml…".to_string(),
                                        ));
                                        terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                                    }

                                    let herdr_bin = plugin::herdr_cli::herdr_bin();
                                    let result = open_config_editor(
                                        &path,
                                        editor_cmd,
                                        &herdr_bin,
                                        |p| open::that(p),
                                    )
                                    .await;

                                    match result {
                                        Ok(()) => app.clear_status(),
                                        Err(message) => {
                                            app.set_status(plugin::app::Status::Error(format!(
                                                "{message}. Edit it manually."
                                            )));
                                        }
                                    }
                                }
                            }
                        }
```

- [ ] **Step 11: Add `editor` to `CONFIG_TEMPLATE`**

In `src/main.rs`, update `CONFIG_TEMPLATE` (around line 200):

```rust
const CONFIG_TEMPLATE: &str = r#"# herdr-linear plugin config. See README.md for the full field reference.

# api_key = "lin_api_..."
# agent_command = "hr"
# editor = "vim"
# team_id = "linear-team-id"

# [project_overrides]
# "repo-name" = "linear-project-id"
"#;
```

- [ ] **Step 12: Run the full test suite and clippy**

Run: `cargo test --all-features 2>&1 | tail -30`
Expected: PASS, all tests (existing `c_key_*`/`modified_c_*`/keybindings tests from PR #31 are
untouched — they test `Action::OpenConfig` generation via `handle_key`, not this handler).

Run: `cargo clippy --all-features --all-targets 2>&1 | tail -20`
Expected: no warnings. If clippy flags the `|p| open::that(p)` closure as
`clippy::redundant_closure` (it may, since `open::that` could be passed directly), replace it
with `open::that` (bare function reference) at that one call site — the test call sites keep
explicit closures since they need to capture `opener_calls`.

- [ ] **Step 13: Commit**

```bash
git add src/main.rs
git commit -m "feat: open config.toml via nvim/configured editor in a herdr pane, OS opener as fallback (TF-614)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

### Task 5: Documentation

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`

**Interfaces:** None (docs only).

- [ ] **Step 1: Update README.md's "Configure" section**

In `README.md`, replace the sentence (around line 308–311):

```
pressing `c` on any Linear error screen (no project matches, multiple projects match, etc.)
opens this file with your OS's default handler for `.toml` files (creating it if it doesn't
exist yet), and the error text itself shows the exact snippet to paste in, with your repo
name already filled in.
```

with:

```
pressing `c` (from any screen — see "Use" below) opens this file, creating it if it doesn't
exist yet; error screens' text itself shows the exact snippet to paste in, with your repo
name already filled in.
```

Then, right after the existing `agent_command` paragraph (after the "(Earlier versions
preferred..." parenthetical, around line 324), add a new paragraph:

```
`c` tries to open `config.toml` in `nvim`, inside a fresh herdr pane, so it's usable over SSH
(e.g. herdr on an iPad) where there's no GUI to hand the file to. Set `editor` in the same
`config.toml` to use a different command instead (a bare binary name, no flags — e.g.
`editor = "vim"`); it's launched the same way, inside a herdr pane. If neither `nvim` nor an
`editor` override is available, or the herdr pane couldn't be opened, `c` falls back to your
OS's default handler for `.toml` files — today's original behavior. Repeated `c` presses reuse
the same editor pane rather than opening a new one each time.
```

- [ ] **Step 2: Update CHANGELOG.md**

In `CHANGELOG.md`, add a new bullet under `[Unreleased] > Added` (create that subsection above
`### Fixed` if it doesn't already exist for this release):

```
- `c` (open `config.toml`) now opens `nvim` inside a herdr pane by default — usable over SSH,
  where the previous OS-default-opener behavior wasn't. Set `editor` in `config.toml` to use a
  different editor instead; if neither resolves or the herdr pane can't be opened, `c` falls
  back to the OS's default opener as before. Repeated `c` presses reuse the same editor pane
  (TF-614)
```

- [ ] **Step 3: Proofread the rendered diff**

Run: `git diff README.md CHANGELOG.md`
Expected: both paragraphs read cleanly in context — no leftover references to "OS's default
handler" as the *only* behavior, no broken sentence flow from the edit.

- [ ] **Step 4: Commit**

```bash
git add README.md CHANGELOG.md
git commit -m "docs: document the editor config key and updated c behavior (TF-614)

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>"
```

---

## Final Verification

- [ ] Run the full suite once more from a clean state: `cargo test --all-features 2>&1 | tail -40`
  — expect every suite green (config.rs: +5 tests, ui.rs: 4 existing tests modified (no new
  functions), editor.rs: +8 tests, main.rs: +10 tests, 0 regressions).
- [ ] Run `cargo clippy --all-features --all-targets 2>&1 | tail -20` — expect no warnings.
- [ ] Manually re-read the new `Action::OpenConfig` handler in `src/main.rs` next to
  `docs/superpowers/specs/2026-08-11-editor-handling-design.md`'s "Data flow / error handling"
  table — confirm every row of that table has a corresponding code path.
