# EDITOR handling for `c` (open config.toml) — design

**Date:** 2026-08-11
**Status:** Approved

## Problem

TF-614 (PR #31) made `c` open `config.toml` from any screen/view state, but the actual "open"
step is still a single `open::that(&path)` call — the OS's default file-association opener. Over
SSH (e.g. herdr on an iPad) there is no GUI, so `open::that` either fails outright or opens
something the user can't see or interact with. There is currently no way to make `c` open the
file in a terminal editor instead.

## Scope

Replace the single `open::that` call in `main.rs`'s `Action::OpenConfig` handler with a
three-tier resolution:

1. **Default:** `nvim`, if found on `PATH`, launched in a new herdr tab/pane targeting the config
   file.
2. **Override:** a new `editor` key in `config.toml` — if set, its value is used as the editor
   command instead of `nvim`, still launched the same way (in a herdr pane).
3. **Fallback:** today's `open::that(&path)` behavior, used whenever neither of the above
   resolves or succeeds (no `nvim` on `PATH`, no `editor` override, or the herdr pane could not be
   opened — e.g. herdr itself isn't running).

Repeated `c` presses reuse the existing editor pane (focus) instead of opening a new one each
time — deliberately a single, global (not per-repo) pane, since `config.toml` is already shared
across all herdr-linear installations (`config.rs`'s existing "per plugin installation, not per
repo" doc comment).

Out of scope: shell-string parsing of the `editor` value (no flags/arguments — a bare binary
name only, consistent with how `agent_command` is *not* used here, since that field already
supports flags for a different purpose); any change to `handle_key`/`Action::OpenConfig`'s
signature (unchanged from PR #31); GUI editors as the herdr-pane target (the herdr-pane path is
inherently terminal-only — a GUI editor override would only ever succeed via tier 3's
`open::that`, since passing e.g. `code` as `editor` would just run `code <path>` inside a herdr
pane, which is a valid but not particularly useful outcome the user can still opt into).

## Architecture

Two new/extended modules, no new `Action` variant:

```
handle_key('c') → Action::OpenConfig(path)                          [unchanged, PR #31]
                          │
                          ▼
        main.rs: OpenConfig handler (event_loop)
        ├─ 1. ensure directory + file exist                          [unchanged]
        └─ 2. open_config_editor(path).await                         [NEW]
               │
               ├─ plugin::editor::resolve_editor_command(...)
               │     config.toml `editor` ──override──▶ "nvim" if on PATH ──none──▶ None
               │
               ├─ Some(cmd) → open_config_in_herdr_pane(herdr_bin, cmd, path).await
               │     ├─ herdr_cli::agent_focus(herdr_bin, "config")  → Ok ⇒ done
               │     └─ Err → tab_create + agent_start(argv=[cmd, path]) + pane_close cleanup
               │
               └─ any failure above (incl. `None`) → open::that(path)  [unchanged fallback]
```

### `src/plugin/config.rs` (extended)

- `ConfigFile` gains `editor: Option<String>`.
- `resolve_editor_override(config_dir: Option<&Path>) -> Result<Option<String>>` — same shape as
  the existing `resolve_agent_command_override` (trim, empty → `None`, malformed TOML → `Err`).
- `load_editor_override()` — env-var-driven wrapper, same pattern as `load_agent_command_override`.
- `ResolvedConfigSummary` gains an `editor` field (consistency with the existing settings-summary
  surface — `agent_command`/`team_id` are already exposed there).

### `src/plugin/editor.rs` (new)

Pure decision logic only — no process/socket access, same charter as `implement.rs`:

- `find_on_path(path_env: Option<&str>, binary: &str) -> Option<PathBuf>` — scans `path_env`
  (injected, not read directly, for testability) via `std::env::split_paths`; checks
  `<dir>/<binary>` (and `<dir>/<binary>.exe` on Windows) for existence. No new dependency (no
  `which` crate).
- `resolve_editor_command(config_editor: Option<String>, path_env: Option<&str>) -> Option<String>`
  — config override first, else `"nvim"` if `find_on_path` locates it, else `None`. Takes already-
  resolved inputs (mirrors the `resolve_*` functions in `config.rs`), so it stays a pure function.
- `const EDITOR_AGENT_NAME: &str = "config"` — deliberately global, not derived from repo/issue;
  documented inline with the reasoning above (shared `config.toml` ⇒ shared pane is correct, not
  an accidental collision).
- `build_editor_argv(editor_cmd: &str, config_path: &Path) -> Vec<String>` →
  `[editor_cmd, config_path.display().to_string()]`.

### `src/plugin/herdr_cli.rs` (extended)

- `pub async fn agent_focus(herdr_bin: &str, target: &str) -> Result<()>` — thin wrapper mirroring
  `pane_close`'s shape (`run` + `interpret_output`, no special-casing of `agent_not_found`: every
  error from this call is treated identically by the caller — "not there, create it").

### `src/main.rs` (extended)

- `async fn open_config_in_herdr_pane(herdr_bin: &str, editor_cmd: &str, config_path: &Path) -> Result<(), String>`:
  1. `agent_focus(herdr_bin, EDITOR_AGENT_NAME)` → `Ok(())` on success.
  2. Otherwise `tab_create(herdr_bin, config_path.parent(), "config")` +
     `agent_start(herdr_bin, EDITOR_AGENT_NAME, cwd, tab_id, argv)`, then close the redundant root
     pane exactly like `implement_one` does (`started.pane_id != created_tab.root_pane_id` guard).
     Any failure in this chain → `Err(message)`.
- `async fn open_config_editor(path: &Path) -> Result<(), String>`:
  1. Resolve the editor command (`config::load_editor_override()` + `PATH` env +
     `editor::resolve_editor_command`).
  2. `Some(cmd)` → try `open_config_in_herdr_pane`; success ⇒ return `Ok(())` (and **do not** also
     call `open::that` — first successful stage wins, never both, to avoid opening the file twice).
  3. Any other outcome (`None`, or the herdr-pane attempt failed) → `open::that(path)`, same error
     message format as today (`"Couldn't open {path}: {e}. Edit it manually."`).
- `OpenConfig` handler: unchanged fs-existence step, then `open_config_editor(&path).await` in
  place of today's direct `open::that(&path)` call. A transient `Status::Ok("Opening
  config.toml…")` + redraw is shown *before* the `.await`, mirroring `Action::Implement`'s
  pattern, so a herdr round-trip doesn't read as a hang.

## Data flow / error handling

Editor resolution (pure, no I/O side effects beyond reading `PATH`/config):

```
config.toml `editor` set?
  yes → that command, regardless of whether nvim is also on PATH
  no  → "nvim" on PATH?
          yes → "nvim"
          no  → None  (straight to tier 3)
```

Execution, staged with silent fallthrough (no status message unless the final stage also fails):

| Stage | Action | On failure |
|---|---|---|
| 1/2 | `agent_focus("config")` | → next |
| 1b  | `tab_create` + `agent_start` | → next |
| 3   | `open::that(path)` | → `Status::Error("Couldn't open {path}: {e}. Edit it manually.")` — unchanged from today |

The chain is "first success wins", not "try all" — once a stage succeeds, later stages are
skipped entirely, so the file is never opened twice.

**Known, accepted race:** two `c` presses in quick succession (before the first `agent_start`
round-trip completes) could both see `agent_focus` fail and both attempt to create a tab, briefly
producing two "config" panes. Not guarded against — self-heals on the next `c` press (one of the
two is then found via `agent_focus`), and is harmless (two nvim instances on the same file; nvim's
own swap-file warning covers it). Same risk class already accepted by `agent_start`'s existing
`agent_name_taken` retry logic for the AI-agent flow.

**herdr unavailable** (not running, socket unreachable): `agent_focus` fails fast (verified live:
`agent focus <nonexistent>` returns `{"error":{"code":"agent_not_found",...}}` immediately, no
timeout wait), `tab_create` then also fails — both cheaply, well under the `DEFAULT_CLI_TIMEOUT`
worst case — and the whole flow falls through to `open::that` with no special-casing needed. No
upfront "are we inside a herdr session" check.

## Testing strategy

**Pure functions (unit tests, no subprocess):**
- `find_on_path` — found / not found / empty `PATH` / `PATH` with nonexistent entries.
- `resolve_editor_command` — override wins over `nvim` even when both resolvable; `nvim` wins with
  no override; `None` when neither resolves.
- `resolve_editor_override` (config.rs) — same coverage shape as the existing
  `resolve_agent_command_override` tests (set / empty / absent / malformed TOML).
- `build_editor_argv`.

**Orchestration (`open_config_in_herdr_pane`, async, fake `herdr` script):**
Extends the existing `write_dispatching_herdr_script` test helper with an `"agent focus"` case:
- `agent focus` succeeds → `Ok(())` immediately; `tab create`/`agent start` must NOT run (reuse
  the existing "`... should not run`" script-guard pattern).
- `agent focus` fails (`agent_not_found`) → `tab create` + `agent start` run, succeed → `Ok(())`.
- `agent focus` fails, `tab create` fails → `Err(...)`.
- `agent focus` fails, `tab create` succeeds, `agent start` fails → `Err(...)` (mirrors the
  existing "possibly orphaned tab" `implement_one` test).
- Redundant root pane closed when `agent_start`'s pane differs from the tab's root pane (mirrors
  the existing `implement_one` pane-close tests, success and failure-as-warning cases).

**End-to-end (`open_config_editor`):**
- No editor resolvable → the (injected/mocked) opener is called, nothing herdr-related is
  attempted.
- Editor resolvable, herdr-pane path succeeds → the opener is **not** called (no double-open).
- Editor resolvable, herdr-pane path fails → fallback to the opener, success there → `Ok(())`, no
  status message.
- Both fail → same error message format as today.

**Unaffected, no changes needed:** all `c_key_*`/`modified_c_*`/keybindings tests from PR #31 —
they test `Action::OpenConfig` *generation* via `handle_key`, not its execution.

## Out of scope / open items for the implementation plan

- Exact wording of the transient `"Opening config.toml…"` status message.
- Whether `ResolvedConfigSummary`'s new `editor` field needs UI surfacing in the settings/help
  view in this PR, or can land as plumbing only or a fast follow — the implementation plan should
  check what `agent_command`/`team_id` currently get before deciding.
- README.md keybindings section already documents `c` as global (from the prior review-fix pass);
  it should gain a one-line mention of the `editor` config key alongside `agent_command`.
