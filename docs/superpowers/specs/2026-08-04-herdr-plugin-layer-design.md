# herdr-linear: Herdr Plugin Layer — Design

**Date:** 2026-08-04
**Status:** Approved for planning

## Purpose

`herdr-linear` is currently a pure-Rust Linear GraphQL client library (Phase 1 of the
project roadmap, complete and tested). This design covers the next step: wrapping that
library in an actual [Herdr](https://herdr.dev) plugin — a `herdr-plugin.toml` manifest
plus a TUI binary — so it can be installed via `herdr plugin install` and used from
inside a Herdr session, matching the original goal of publishing to
https://herdr.dev/docs/plugins/.

## Scope (v1)

**In scope:**
- Read-only "My Issues" panel: list the authenticated user's assigned issues, view
  details, open the selected issue in the browser.
- Two entry points: open as a split pane, or open in its own tab — both idempotent
  (press again to focus/close/switch rather than duplicate).
- Config-file based API key, with an environment-variable fallback.

**Explicitly out of scope for v1** (deferred, not forgotten — captured here so later
work doesn't silently expand this plan):
- Changing issue status, creating issues, adding comments from the plugin UI (the
  library already supports these calls; the plugin UI does not expose them yet).
- Team/project/cycle browsing views — v1 is "my issues" only.
- Prebuilt-binary distribution with checksum verification (like herdr-file-viewer's
  `fetch-or-build.sh`) — v1's `[[build]]` step is a plain `cargo build --release`,
  requiring a Rust toolchain at install time. Binary distribution is Phase 4
  ("Distribution") territory per ROADMAP.md.
- Windows support — v1 targets linux/macos only (no Windows `[[build]]`/`[[actions]]`
  variants, unlike the herdr-file-viewer reference).
- Automated end-to-end tests against a live Herdr instance — verified manually via
  `herdr plugin link` for v1.

## Reference material

Two existing Herdr plugins informed this design:
- `JacquesvanWyk/herdr-linear` (the project's original inspiration, which didn't work
  in the user's environment): shells out to the `lc` (linearctl) npm CLI, fzf, and jq.
  Its UX ideas (My Todos menu, split-pane + own-tab entry points, change-status-from-picker)
  are a useful reference, but its architecture is deliberately not reused — this project's
  standing goal is a dependency-free, pure-Rust plugin.
- `herdr-file-viewer` (installed locally, `~/.config/herdr/plugins/github/herdr-file-viewer-*`):
  a mature, production Rust/ratatui Herdr plugin. Its manifest structure, launch-decision
  toggle logic, and pane-identification approach are reused directly (see below) — verified
  against this machine's real `herdr pane list` socket output and Herdr 0.7.3.

## Architecture

The plugin layer lives in the existing `herdr-linear` crate (not a new repo). A new
binary — replacing the current placeholder `src/main.rs` — becomes the Herdr plugin
process: a small ratatui/crossterm TUI that uses `LinearClient` directly. The existing
`cli` Cargo feature is renamed to `plugin` (same purpose — an optional, non-default
build target — but now gating `ratatui`/`crossterm`/`toml` instead of `clap`). The
current `main.rs` demo content moves into `examples/`, since it substantially overlaps
with `examples/basic_usage.rs`.

New dependencies, all gated behind the `plugin` feature (the default library build stays
dependency-free of them): `ratatui`, `crossterm`, `toml` (config parsing — `serde` and
`serde_json` are already default dependencies), and `open` (cross-platform "open URL in
browser").

### Manifest (`herdr-plugin.toml`)

```toml
id = "herdr-linear"
name = "herdr-linear"
version = "0.1.0"
min_herdr_version = "0.7.0"
platforms = ["linux", "macos"]

[[build]]
command = ["cargo", "build", "--release", "--features", "plugin"]

[[panes]]
id = "linear-panel"
title = "Linear"
placement = "split"
command = ["./target/release/herdr-linear"]

[[actions]]
id = "open-split"
title = "Open Linear panel"
command = ["bash", "scripts/open-split.sh"]

[[actions]]
id = "open-tab"
title = "Open Linear panel (tab)"
command = ["bash", "scripts/open-tab.sh"]
```

Keybindings are configured separately, in the user's own `~/.config/herdr/config.toml`,
via `[[keys.command]]` entries referencing `herdr-linear.open-split` /
`herdr-linear.open-tab` — the plugin does not prescribe specific keys.

### Components

New modules, added to the existing crate under a `plugin` Cargo feature:

- `src/plugin/config.rs` — resolves the Linear API key: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`
  (`api_key = "..."`) first, then `LINEAR_API_KEY` env var, then a graceful "not configured"
  state (see Error Handling). Never logs the resolved key.
- `src/plugin/launch.rs` — pure, unit-testable functions `launch_decision(pane_list_json) -> String`
  and `launch_decision_tab(pane_list_json) -> String`, returning one of `OPEN`, `FOCUS <pane_id>`,
  `CLOSE <pane_id>`, `SWITCHTAB <tab_id>`.
- `src/plugin/app.rs` — the ratatui application state: issue list, selected index,
  loading/error state, and the key-handling event loop (↑/↓ navigate, Enter view detail,
  `o` open in browser, `q`/Esc quit, `r` retry on error).
- `src/plugin/ui.rs` — rendering: a list widget plus a detail panel.
- `src/main.rs` — the binary entry point. Dispatches on argv: `--launch-decision` /
  `--launch-decision-tab` read a `pane list` JSON blob from stdin and print the decision
  (for the launcher scripts below), exiting immediately; otherwise it starts the Tokio
  runtime, builds a `LinearClient`, and runs the TUI event loop.
- `scripts/open-split.sh` / `scripts/open-tab.sh` — idempotent launchers, structurally
  identical to herdr-file-viewer's: call `$HERDR_BIN_PATH pane list`, pipe the result into
  `herdr-linear --launch-decision[-tab]`, and act on the printed decision via
  `herdr plugin pane open` / `herdr pane zoom` / `herdr pane close` / `herdr tab focus`.

### Data flow (split entry point; the tab variant is analogous)

1. User presses the bound key → Herdr runs the `open-split` action → `scripts/open-split.sh`.
2. The script calls `$HERDR_BIN_PATH pane list` and pipes the JSON into
   `herdr-linear --launch-decision`.
3. On `OPEN`, the script runs `herdr plugin pane open --plugin herdr-linear --entrypoint
   linear-panel --placement split --focus`. Herdr applies the manifest's pane `title`
   ("Linear") as the new pane's `label` automatically.
4. The plugin binary starts, resolves the API key via `plugin::config`, calls
   `client.get_viewer()` to get the current user's id, then `client.get_issues(...)` with
   an assignee filter for that id — `LinearClient` has no dedicated "my issues" helper today,
   so the plugin composes it from the two existing calls.
5. The TUI renders the issue list. `o` opens `issue.url` in the system browser (via the
   `open` crate, which already handles the linux/macos launcher differences this v1 targets);
   `q`/Esc quits (which naturally closes the pane, since it was foreground).
6. On a second key press while the panel is already open, `launch_decision` returns `FOCUS`
   or `CLOSE` instead of `OPEN` — see Pane identification below.

### Pane identification and toggle logic

Verified against this machine's real `herdr pane list` output and Herdr 0.7.3: panes opened
via `plugin pane open --entrypoint <id>` get their manifest `title` applied as the pane's
`label`, so `pane list` results can be matched on `label == "Linear"` without any custom
tagging protocol.

- **Split variant** (`launch_decision`): if no pane in the response is `focused`, the current
  tab is unknowable → `OPEN` (never guess). Otherwise, look for a `label == "Linear"` pane in
  the *focused pane's tab*: if that pane *is* the focused one → `CLOSE` (toggle off); if it
  exists but isn't focused → `FOCUS`; if it isn't in this tab at all → `OPEN`.
- **Tab variant** (`launch_decision_tab`): same in-tab `CLOSE`/`FOCUS` check first. Failing
  that, look for a `label == "Linear"` pane in *another tab of the same workspace* →
  `SWITCHTAB <tab_id>` (never jump across workspaces — a panel in a different workspace is
  left alone and a fresh one opens here).
- Any pane/tab id returned by `launch_decision*` is validated as "flag-safe" before the
  launcher script ever passes it to `herdr pane zoom|close` / `herdr tab focus` — a
  malformed or hostile id from `pane list` can never option-inject into those commands.

### Config & error handling

- Missing/empty API key (both config file and env var): the TUI shows an inline error state
  — "No Linear API key found. Set `api_key` in `<resolved config path>` or export
  `LINEAR_API_KEY`." — using the actual resolved path, not a generic message. It does not
  crash or exit.
- Any `LinearClient` error (`AuthenticationFailed`, `NetworkError`, `RateLimitExceeded`,
  `GraphQLError`, etc. — already typed by the existing `Error` enum) replaces the list/detail
  content with an inline error message and an `r` = retry hint.
- `launch_decision`/`launch_decision_tab`: any parse or logic failure degrades to `OPEN` —
  never a crash, never a silent no-op.
- A panic hook restores the terminal (raw mode off, leave alternate screen) before any panic
  propagates, so a crash can't leave the Herdr pane stuck in a broken terminal state.

### Testing

- `plugin::launch`: unit tests feeding synthetic `pane list` JSON literals covering OPEN /
  FOCUS / CLOSE / SWITCHTAB / malformed-JSON / unsafe-id cases — no real Herdr socket needed.
- `plugin::config`: precedence tests (file present, file absent + env set, neither set) using
  a temp directory as `HERDR_PLUGIN_CONFIG_DIR`.
- `plugin::app`: state-transition unit tests (navigation, opening with an empty list, retry
  after an error) without a real terminal.
- Issue fetching reuses the already mockito-tested `LinearClient` — no new HTTP-layer test
  infrastructure needed.
- No automated end-to-end test against a live Herdr instance in v1; verified manually via
  `herdr plugin link <path>` during development, matching how herdr-file-viewer treats its
  own FFI/glue code.

## Open items for the implementation plan

- Exact ratatui widget layout (list vs. detail split ratio, styling) — visual detail, not a
  design-level decision.
- Whether `plugin::launch`'s `Pane` struct reads `workspace_id` directly from `pane list`
  JSON (present in the real output observed) rather than parsing it from an id prefix as
  herdr-file-viewer does — an implementation simplification, not a behavior change.
