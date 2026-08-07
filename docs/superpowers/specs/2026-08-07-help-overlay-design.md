# In-app Help overlay (`?` key) — design

**Date:** 2026-08-07
**Status:** Approved
**Linear:** [TF-585](https://linear.app/talent-factory/issue/TF-585)

## Problem

The plugin has no in-TUI way to see what's changed recently, what keys do what, what
config is currently in effect, or basic "about" info (version, repo, license) — all of
that currently requires leaving the terminal (README, CHANGELOG, config.toml, GitHub).
TF-585 asks for a `?`-triggered help overlay with four tabs (What's New / Keybindings /
Settings / About), inspired by [smarzban/herdr-file-viewer](https://github.com/smarzban/herdr-file-viewer)'s
own `?` overlay — a real reference screenshot of that plugin's Settings tab was used
during design (see below) and pins the exact visual convention this design follows:
a `Help: <tabs>` bordered block with the active tab marked, scrollable body, and a
footer line naming the controls.

Reference screenshot (herdr-file-viewer, Settings tab active):

```
┌Help: What's New   Keybindings   > Settings   About──────┐
│                                                          │
│ Config: no file found, using defaults.                  │
│ Location:                                                │
│ /Users/daniel/.config/herdr/plugins/config/herdr-file... │
│                                                          │
│ editor          = nvim                                  │
│ open             = open                                 │
│ ...                                                      │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ Tab/↔ switch · 1-9 jump · j/k scroll · Esc/q/? close      │
└──────────────────────────────────────────────────────────┘
```

## Scope

- `?` toggles a read-only overlay from anywhere in the plugin (menu or any loaded/error
  view), except while type-to-filter (`/`) is actively capturing keystrokes, where `?`
  must still type into the filter query instead.
- Four tabs — What's New, Keybindings, Settings, About — switchable via `Tab`/`←`/`→`
  or number keys `1`-`4`, scrollable via `j`/`k`/`↑`/`↓`, closable via `Esc`/`q`/`?`.
- Keybindings tab content comes from one new canonical registry, not a hand-duplicated
  copy of `handle_key`'s match arms.
- Settings tab shows the plugin's currently-resolved `config.toml` values, masking
  `api_key` to a set/not-set flag (see "Settings tab" below — the one place this design
  deliberately diverges from the herdr-file-viewer reference, which has no secrets in
  its own config).
- What's New reads `CHANGELOG.md`'s `[Unreleased]` section, embedded at compile time.
  `CHANGELOG.md` itself is backfilled as part of this ticket (see "CHANGELOG.md
  backfill" below) — today it only has generic library-level entries from the initial
  `0.1.0` release and doesn't mention any of the plugin UI work shipped since.
- About tab shows plugin name, version, repo URL, license — all from `Cargo.toml`
  build-time env vars, no new data source.
- README gets a mention of `?` alongside the plugin's other documented keybindings.

Out of scope: persisting overlay state (tab/scroll) across plugin restarts; a search/filter
within the overlay itself (unlike herdr-file-viewer's Settings tab, which has one — not
asked for by the AC); any automated test asserting the Keybindings registry stays in sync
with `handle_key`'s actual match arms (see "Keybindings registry" below — this mirrors the
`hp41-calculator-emulator` precedent, which has no such test either).

## Architecture

No new `Screen` variant, no changes to `ViewState`/`FilterState`, no changes to any
existing `handle_key` match arm. Five pieces, all additive:

### `src/plugin/app.rs` (extended)

- New `pub struct HelpOverlayState { pub tab: HelpTab, pub scroll: u16 }` and
  `pub enum HelpTab { WhatsNew, Keybindings, Settings, About }`, with `HelpTab::index()`
  / `HelpTab::from_index()` (for `1`-`4` jump) and `HelpTab::next()`/`HelpTab::prev()`
  (for `Tab`/arrow cycling, wrapping).
- New field on `App`: `help_overlay: Option<HelpOverlayState>` (sibling to `screen`,
  `status` — `None` means closed). New accessor `App::help_overlay(&self)`, and methods
  `open_help_overlay`, `close_help_overlay`, `help_overlay_switch_tab`,
  `help_overlay_jump_tab`, `help_overlay_scroll` mirroring the existing style of
  `move_menu_selection_down`/`start_filtering`/etc. (small, single-purpose mutators
  called from `handle_key`).
- `handle_key` gains exactly two new early checks, both before the existing `in_menu`
  branch:
  1. Right after the existing `Ctrl+C` check: if `app.help_overlay().is_some()`,
     dispatch to a new `handle_help_overlay_key(app, key)` (Tab/←/→ switch, `1`-`4`
     jump, `j`/`k`/↑/↓ scroll, `Esc`/`q`/`?` close) and return — the overlay owns all
     input while open, taking priority over menu nav, view nav, and in-progress
     filtering alike.
  2. Immediately after: if `key == KeyCode::Char('?')` and `!app.is_filtering()`, call
     `app.open_help_overlay()` and return `None`. Gated on `is_filtering()` so `?`
     still types into an active filter query instead of opening the overlay — the same
     "text-input capture wins" exception the `hp41-calculator-emulator` project uses
     for its own `?` overlay vs. its text-input modals. No exception needed for
     `is_view_error()`: opening from the error screen is fine, there's no text capture
     to protect there.

### `src/plugin/keybindings.rs` (new)

- `pub struct KeyBinding { pub keys: &'static str, pub action: &'static str, pub context: &'static str }`
  and `pub static KEYBINDINGS: &[KeyBinding]`, one entry per binding currently
  documented in `handle_key`'s doc comments and the README's "Use" section (menu nav,
  view nav, `/` filter, `o` open, `<Space>` mark, `<Enter>` implement, `r` retry, `c`
  open config, `Esc`/`q` quit/back, plus the new `?` binding itself), grouped by
  `context` ("Menu", "View", "Filtering", "Error screen", "Global") for rendering.
  This is the single place bindings are described in prose for display purposes;
  `handle_key` stays exactly as it is today, hand-written match arms with their
  existing precedence-explaining doc comments. Mirrors the `hp41-calculator-emulator`
  precedent directly: that project's `?` overlay content is driven by one canonical
  data source (`docs/hp41cv-functions.json` there, `KEYBINDINGS` here) while the actual
  input-dispatch code (`key_to_op`/`handle_key` there, `handle_key` here) stays
  hand-maintained and untouched — including that precedent's choice not to add an
  automated equivalence check between the two. Drift risk is accepted and mitigated the
  same way doc-comment drift already is in this codebase: by hand, at review time.

### `src/plugin/config.rs` (extended)

- `ConfigFile`'s fields are currently private to the module (only resolved through
  `resolve_*` free functions, each of which independently calls `read_config_file` and
  propagates its `Err` via `?` — meaning a config.toml that fails to parse makes
  `resolve_api_key`/`resolve_agent_command_override`/`resolve_team_id_override` **all**
  fail identically, not fall back to a partial result). Rather than compose those three
  public wrappers (which would mean either surfacing the same "invalid TOML" message
  three times or silently swallowing three `Err`s), add
  `pub(crate) fn resolved_summary(config_dir: Option<&Path>, env_api_key: Option<&str>) -> ResolvedConfigSummary`
  that calls `read_config_file(config_dir)` exactly once and matches on its
  `Result<Option<ConfigFile>>` directly:
  - `Ok(None)` → `ConfigFileStatus::NotFound`; `api_key_set` from `env_api_key` alone
    (nothing to combine with — the same "config missing, env var still resolves it"
    situation `resolve_api_key` allows); `agent_command`/`team_id` `None`,
    `project_overrides` empty.
  - `Ok(Some(file))` → `ConfigFileStatus::Found`; `api_key_set` true if `file.api_key`
    is non-empty *or* `env_api_key` is (mirrors `resolve_api_key`'s own precedence);
    `agent_command`/`team_id` taken from the file with the same
    trim-and-treat-empty-as-unset rule `resolve_agent_command_override`/
    `resolve_team_id_override` already apply; `project_overrides` copied as-is.
  - `Err(e)` → `ConfigFileStatus::Invalid(e.to_string())`; every other field left at its
    empty/`None`/`false` default — accurately reflecting that the plugin can't resolve
    *anything* from a broken config.toml either (every `resolve_*` call the plugin
    actually makes would fail the same way), not a partial/fallback state.

  Returns `pub(crate) struct ResolvedConfigSummary { pub path: String, pub status: ConfigFileStatus, pub api_key_set: bool, pub agent_command: Option<String>, pub team_id: Option<String>, pub project_overrides: BTreeMap<String, String> }`,
  `pub(crate) enum ConfigFileStatus { NotFound, Found, Invalid(String) }`. `path` is
  `config_path_hint(config_dir)` (existing function, reused as-is). The call site
  (`ui.rs`, at render time) builds `config_dir`/`env_api_key` the same way `load()`
  does: `std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from)` (`.as_deref()`)
  and `std::env::var("LINEAR_API_KEY").ok()`. One `read_config_file` call, one place the
  three-way status is decided — the Settings tab has no independent config-reading code
  path to drift from the real one the plugin uses to connect.

### `src/plugin/ui.rs` (extended)

- `draw()` renders the current `Screen` exactly as today, then — if
  `app.help_overlay().is_some()` — calls a new `draw_help_overlay(frame, app)`: `Clear`
  + a centered `Rect` (~80% × 90% of the frame, matching the reference screenshot's
  proportions), a bordered block titled `Help: What's New  Keybindings  Settings  About`
  with the active tab marked (`>` prefix + bold, per the reference), the active tab's
  content in the scrollable body (a `Paragraph` with `.scroll((state.scroll, 0))`), and
  a footer line `Tab/←→ switch · 1-4 jump · j/k scroll · Esc/q/? close`.
- Each tab's content is built by a small private function
  (`whats_new_lines`/`keybindings_lines`/`settings_lines`/`about_lines`) returning
  `Vec<Line>`, so each is independently testable without rendering (see "Testing
  strategy").

### `CHANGELOG.md` backfill

Add plugin-facing entries to `[Unreleased]` for the recent UI work that shipped without
a CHANGELOG entry, plus one for this feature:

```markdown
## [Unreleased]
### Added
- In-app Help overlay (`?` key): What's New / Keybindings / Settings / About (TF-585)
- Type-to-filter the loaded issue list by title/identifier (TF-580)
- Guaranteed tab-per-issue on the Linear implement flow (TF-579)
- Unique per-issue herdr agent names + multi-select issues (TF-590)
```

placed above the existing generic library-level entries in that section (which stay,
unless the implementation plan finds a cleaner reorganization).

## Data flow — opening and rendering the overlay

```
handle_key('?')  [overlay closed, not filtering]
  └─ app.open_help_overlay()             -> help_overlay = Some({ tab: WhatsNew, scroll: 0 })

event_loop's next terminal.draw(...)
  └─ ui::draw(frame, app)
       ├─ draw_menu / draw_view(...)      (unchanged — renders the untouched underlying screen)
       └─ draw_help_overlay(frame, app)   [NEW - only when help_overlay.is_some()]
            ├─ Clear(overlay_area)
            ├─ tab bar (HelpTab::ALL, active marked)
            ├─ whats_new_lines() | keybindings_lines() | settings_lines() | about_lines()
            └─ footer

handle_key(<any key>)  [overlay open]
  └─ handle_help_overlay_key(app, key)
       ├─ Tab / → / ←   -> app.help_overlay_switch_tab(...), scroll reset to 0
       ├─ '1'..'4'      -> app.help_overlay_jump_tab(...),   scroll reset to 0
       ├─ 'j' / ↓        -> app.help_overlay_scroll(+1)
       ├─ 'k' / ↑        -> app.help_overlay_scroll(-1)
       └─ Esc / 'q' / '?' -> app.close_help_overlay()   (help_overlay = None; screen underneath unchanged)
```

`whats_new_lines()` parses the embedded `CHANGELOG.md` (`include_str!("../../CHANGELOG.md")`)
by locating the `## [Unreleased]` heading and collecting lines up to the next `## `
heading (or falls back to the first versioned section if `[Unreleased]` is empty or
missing — a `CHANGELOG.md` in an unexpected shape shows a short "couldn't parse
CHANGELOG.md" line in the tab rather than panicking, since — unlike `hp41-calculator-emulator`'s
`docs/hp41cv-functions.json`, which is schema-validated data with an intentional
hard-build-blocker on malformed input — `CHANGELOG.md` is freeform prose maintained by
hand and shouldn't be able to break the build). The heading line shown above the parsed
entries is synthesized as `vX.Y.Z (unreleased)` using `env!("CARGO_PKG_VERSION")`.

## Settings tab

Matches the herdr-file-viewer reference's raw `key = value` dump style, with one
deliberate divergence: `api_key` is the one credential in this plugin's config (the
reference plugin's config has none), so it never renders as a raw string — only as
`✓ Set` / `✗ Not set`. Everything else renders exactly as resolved:

```
Config: found
Location: ~/.config/herdr/plugins/config/herdr-linear/config.toml
api_key          = ✓ Set
agent_command    = claude (default)
team_id          = 019... (or "not set")
project_overrides:
  herdr-linear   = 5b05b96c-...
```

"(default)" is appended to `agent_command` when unset (mirrors the actual fallback
chain documented in the README: explicit config → other open tabs → `"hr"`) so the tab
doesn't just say "not set" for a value that in practice is always resolved to
*something* at implement-time.

## Error handling

- `ConfigFileStatus::NotFound`: `Config: no file found, using defaults.` (matches the
  reference screenshot's exact wording), all fields shown with their built-in
  defaults/`Not set`.
- `ConfigFileStatus::Invalid(message)`: `Config: <path> exists but is invalid — <message>`
  (reuses `read_config_file`'s existing `Error::ConfigError` text, not a new message);
  every other field shows `Not set`/empty rather than a guess, since a broken
  config.toml means every `resolve_*` call the plugin actually makes to connect would
  fail the same way — there is no partial/fallback result to show instead.
- `ConfigFileStatus::Found`: `Config: found`, fields populated from the parsed file.
- `CHANGELOG.md` missing its `[Unreleased]` section or unparsable: fall back to the
  newest versioned section; if that's also absent, show a one-line "couldn't find
  recent changes" message rather than panicking or leaving the tab blank.
- Terminal too small for the ~80%×90% overlay: not specially handled — same as every
  other view in this plugin today (no existing minimum-terminal-size handling anywhere
  in `ui.rs`), so this doesn't introduce a new class of untreated failure.

## Testing strategy

- `app.rs`: unit tests for `HelpTab::next`/`prev`/`from_index` (wrapping at both ends),
  `handle_help_overlay_key` dispatch for every key (switch/jump/scroll/close), and the
  `?`-while-filtering exception (`?` pressed during `is_filtering()` pushes into the
  query, does not open the overlay; `?` pressed while not filtering opens it from both
  `Screen::Menu` and `Screen::View`).
- `keybindings.rs`: a test asserting `KEYBINDINGS` is non-empty and every entry has
  non-empty `keys`/`action`/`context` — a cheap guard against an accidentally-empty
  table, not a drift check against `handle_key` (see Scope: no such check is added, by
  design, matching the `hp41-calculator-emulator` precedent).
- `config.rs`: unit tests for `resolved_summary` — all three `ConfigFileStatus`
  outcomes (not found, found, invalid TOML), `api_key_set` true/false from each of
  config-file-only / env-only / both / neither, and `project_overrides` round-tripping,
  reusing the module's existing temp-dir test fixtures.
- `ui.rs`: unit tests for `whats_new_lines()`'s CHANGELOG parser against fixture
  strings (`[Unreleased]` present with entries, `[Unreleased]` present but empty,
  section missing entirely, malformed heading) — pure-function tests, no terminal
  rendering involved, matching how `MarkdownStyleSheet`/other `ui.rs` logic is already
  tested in that module's `tests` submodule.
- No snapshot/visual test for `draw_help_overlay` itself — consistent with this
  codebase's existing style (no other view has one either).

## Out of scope / open items for the implementation plan

- Whether `KEYBINDINGS` should also be consumed to generate part of the README's "Use"
  section (avoiding *that* duplication too) — not attempted here; README is updated by
  hand for this ticket, same as it already is today for every other keybinding.
- Per-tab remembered scroll position (currently reset to `0` on every tab switch) — not
  requested by the AC, left as a possible future refinement.
