# In-app Help overlay (`?` key) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `?`-triggered, read-only help overlay (What's New / Keybindings / Settings / About) to the herdr-linear plugin TUI, per TF-585.

**Architecture:** `App` gains one new `Option<HelpOverlayState>` field, rendered as a `Clear`-and-centered-`Rect` layer on top of whatever screen is already showing — no new `Screen` variant, no changes to any existing `handle_key` match arm. `handle_key` gains exactly two new early checks (overlay-active dispatch, and `?`-opens-overlay). Content for the four tabs comes from four independent pure functions in `ui.rs`, backed by a new canonical keybindings registry (`keybindings.rs`) and a new config-summary accessor (`config.rs`).

**Tech Stack:** Rust 2021 (rust-version 1.88), ratatui 0.30.1, crossterm 0.29.0, tempfile (dev-dependency, tests only).

**Spec:** `docs/superpowers/specs/2026-08-07-help-overlay-design.md` — read it first for the full rationale (hp41-calculator-emulator precedent, herdr-file-viewer reference screenshot).

## Global Constraints

- Work happens in the worktree at `.worktrees/TF-585` on branch `feature/tf-585-in-app-help-overlay-taste-whats-new-keybindings-settings` — already created and verified (clean `cargo build --all-features` + `cargo test --all-features`, 296 passing). Every command below assumes that directory as cwd.
- Commit style: plain Conventional Commits (`feat:`, `test:`, `docs:`, `chore:`), no emoji — matches this repo's actual git log (`git log --oneline`). Reference `TF-585` in each feature commit's subject.
- Run `cargo test --all-features -- --nocapture` (or `just test`) after every implementation step before committing. Run `just check` (fmt + clippy + test) before the final task's commit at minimum.
- No new dependencies — everything needed (`ratatui`, `crossterm`, `tempfile` for tests) is already in `Cargo.toml`.
- Every new `pub`/`pub(crate)` item gets a doc comment, matching every existing item in `app.rs`/`ui.rs`/`config.rs`.
- Never touch an existing `handle_key` match arm, `Screen`/`ViewState`/`FilterState` variant, or any `resolve_*`/`load_*` function in `config.rs` — every task in this plan is additive.

---

### Task 1: Keybindings registry

**Files:**
- Create: `src/plugin/keybindings.rs`
- Modify: `src/plugin/mod.rs` (add `pub mod keybindings;`)

**Interfaces:**
- Produces: `pub struct KeyBinding { pub keys: &'static str, pub action: &'static str, pub context: &'static str }`, `pub static KEYBINDINGS: &[KeyBinding]` — consumed by Task 7's `ui::keybindings_lines`.

- [ ] **Step 1: Write the module with its registry table and test**

Create `src/plugin/keybindings.rs`:

```rust
//! Canonical registry of the plugin's keybindings, rendered by the help overlay's
//! Keybindings tab (`?`, TF-585). This is the only place bindings are described in
//! prose — `app.rs::handle_key`'s match arms stay hand-written and untouched by this
//! table, mirroring the precedent this design follows (`hp41-calculator-emulator`'s `?`
//! overlay: one canonical data source drives display, the actual input-dispatch code
//! stays hand-maintained). Keeping this table in sync with `handle_key` is a
//! hand-maintenance discipline, not an enforced one — see
//! `docs/superpowers/specs/2026-08-07-help-overlay-design.md`'s Scope section for why no
//! automated drift check is added.

/// One documented keybinding: the key(s) that trigger it, what it does, and which
/// screen/mode it applies in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    /// The key(s), as shown to the user (e.g. `"↑ / ↓"`, `"<Enter>"`, `"c"`).
    pub keys: &'static str,
    /// What the key does, in one short phrase (e.g. `"Move selection"`).
    pub action: &'static str,
    /// Which screen/mode this binding applies in. Entries sharing a `context` must stay
    /// contiguous — `ui::keybindings_lines` groups by context via a single pass over
    /// this table, not a sort, so the table's own order is what's displayed.
    pub context: &'static str,
}

/// Every keybinding currently implemented in `app::handle_key`, grouped by context in
/// the order `handle_key` itself checks them (menu, view, filtering, error-screen-only,
/// then global).
pub static KEYBINDINGS: &[KeyBinding] = &[
    KeyBinding { keys: "↑ / ↓", action: "Move selection", context: "Menu" },
    KeyBinding { keys: "<Enter>", action: "Open highlighted view", context: "Menu" },
    KeyBinding { keys: "q / Esc", action: "Quit", context: "Menu" },
    KeyBinding { keys: "↑ / ↓", action: "Move selection", context: "View" },
    KeyBinding { keys: "/", action: "Filter issues by title/identifier", context: "View" },
    KeyBinding { keys: "o", action: "Open selected issue in browser", context: "View" },
    KeyBinding { keys: "<Space>", action: "Mark/unmark issue for multi-select", context: "View" },
    KeyBinding { keys: "<Enter>", action: "Implement selected (or every marked) issue", context: "View" },
    KeyBinding { keys: "Esc", action: "Back to menu", context: "View" },
    KeyBinding { keys: "q", action: "Quit", context: "View" },
    KeyBinding { keys: "<Enter>", action: "Confirm filter", context: "Filtering" },
    KeyBinding { keys: "Esc", action: "Cancel filter, restore full list", context: "Filtering" },
    KeyBinding { keys: "Backspace", action: "Remove last filter character", context: "Filtering" },
    KeyBinding { keys: "r", action: "Retry", context: "Error screen" },
    KeyBinding { keys: "c", action: "Open config.toml", context: "Error screen" },
    KeyBinding { keys: "?", action: "Toggle this help overlay", context: "Global" },
    KeyBinding { keys: "Ctrl+C", action: "Quit", context: "Global" },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybindings_is_non_empty_and_every_entry_has_no_blank_fields() {
        assert!(!KEYBINDINGS.is_empty());
        for binding in KEYBINDINGS {
            assert!(!binding.keys.is_empty(), "empty `keys` in {binding:?}");
            assert!(!binding.action.is_empty(), "empty `action` in {binding:?}");
            assert!(!binding.context.is_empty(), "empty `context` in {binding:?}");
        }
    }

    #[test]
    fn entries_sharing_a_context_are_contiguous() {
        // Guards the invariant `ui::keybindings_lines`'s single-pass grouping depends
        // on: once a context has appeared and a *different* context follows it, that
        // first context must never reappear later in the table.
        let mut seen = std::collections::HashSet::new();
        let mut last_context = None;
        for binding in KEYBINDINGS {
            if last_context != Some(binding.context) {
                assert!(
                    seen.insert(binding.context),
                    "context {:?} is not contiguous in KEYBINDINGS",
                    binding.context
                );
                last_context = Some(binding.context);
            }
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `src/plugin/mod.rs`, add `pub mod keybindings;` (alphabetically, after `pub mod implement;` and before `pub mod launch;`), and update the file's top doc comment to mention it alongside the other submodules.

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib plugin::keybindings -- --nocapture`
Expected: PASS, 2 tests.

- [ ] **Step 4: Commit**

```bash
git add src/plugin/keybindings.rs src/plugin/mod.rs
git commit -m "feat: add canonical keybindings registry (TF-585)"
```

---

### Task 2: `HelpTab`

**Files:**
- Modify: `src/plugin/app.rs` (new enum + impl, inserted after the `Status` `impl` block, before `pub struct App`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub enum HelpTab { WhatsNew, Keybindings, Settings, About }` with `pub const ALL: [HelpTab; 4]`, `pub fn index(self) -> usize`, `pub fn from_index(index: usize) -> Option<Self>`, `pub fn title(self) -> &'static str`, `pub fn next(self) -> Self`, `pub fn prev(self) -> Self`, and `impl Default for HelpTab` (`WhatsNew`). Consumed by Task 3 (`HelpOverlayState`), Task 4 (`handle_help_overlay_key`), Task 10 (`draw_help_overlay`).

- [ ] **Step 1: Write the failing tests**

Add to the end of `src/plugin/app.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn help_tab_default_is_whats_new() {
        assert_eq!(HelpTab::default(), HelpTab::WhatsNew);
    }

    #[test]
    fn help_tab_index_and_from_index_round_trip_for_every_tab() {
        for tab in HelpTab::ALL {
            assert_eq!(HelpTab::from_index(tab.index()), Some(tab));
        }
    }

    #[test]
    fn help_tab_from_index_out_of_range_returns_none() {
        assert_eq!(HelpTab::from_index(4), None);
    }

    #[test]
    fn help_tab_next_cycles_forward_and_wraps_from_the_last_tab() {
        assert_eq!(HelpTab::WhatsNew.next(), HelpTab::Keybindings);
        assert_eq!(HelpTab::Keybindings.next(), HelpTab::Settings);
        assert_eq!(HelpTab::Settings.next(), HelpTab::About);
        assert_eq!(HelpTab::About.next(), HelpTab::WhatsNew);
    }

    #[test]
    fn help_tab_prev_cycles_backward_and_wraps_from_the_first_tab() {
        assert_eq!(HelpTab::WhatsNew.prev(), HelpTab::About);
        assert_eq!(HelpTab::About.prev(), HelpTab::Settings);
    }

    #[test]
    fn help_tab_title_is_non_empty_for_every_tab() {
        for tab in HelpTab::ALL {
            assert!(!tab.title().is_empty());
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib plugin::app::tests::help_tab -- --nocapture`
Expected: FAIL to compile — `cannot find type `HelpTab` in this scope`.

- [ ] **Step 3: Implement `HelpTab`**

Insert into `src/plugin/app.rs`, immediately after the closing `}` of `impl Status { ... }` and before `/// The main application state container.` (the `App` struct's doc comment):

```rust
/// The four tabs of the in-app help overlay (`?` — TF-585): What's New, Keybindings,
/// Settings, About, in the order they're shown left to right and jumped to via `1`-`4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpTab {
    /// Recent changes, pulled from `CHANGELOG.md`'s `[Unreleased]` section.
    #[default]
    WhatsNew,
    /// Every currently-implemented keybinding, from the canonical registry in
    /// `crate::plugin::keybindings`.
    Keybindings,
    /// The plugin's currently-resolved `config.toml` values.
    Settings,
    /// Plugin name, version, repo, license.
    About,
}

impl HelpTab {
    /// All four tabs, in display/jump order.
    pub const ALL: [HelpTab; 4] = [
        HelpTab::WhatsNew,
        HelpTab::Keybindings,
        HelpTab::Settings,
        HelpTab::About,
    ];

    /// The tab's position in [`Self::ALL`] (`0`-based) — one less than the `1`-`4` jump
    /// key that selects it (see `handle_help_overlay_key`).
    pub fn index(self) -> usize {
        match self {
            HelpTab::WhatsNew => 0,
            HelpTab::Keybindings => 1,
            HelpTab::Settings => 2,
            HelpTab::About => 3,
        }
    }

    /// The tab at `index` within [`Self::ALL`], or `None` if out of range.
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The label shown in the overlay's tab bar.
    pub fn title(self) -> &'static str {
        match self {
            HelpTab::WhatsNew => "What's New",
            HelpTab::Keybindings => "Keybindings",
            HelpTab::Settings => "Settings",
            HelpTab::About => "About",
        }
    }

    /// The next tab in display order (`Tab`/`→`), wrapping from `About` back to `WhatsNew`.
    pub fn next(self) -> Self {
        Self::from_index((self.index() + 1) % Self::ALL.len()).expect("index is always in range")
    }

    /// The previous tab in display order (`←`), wrapping from `WhatsNew` back to `About`.
    pub fn prev(self) -> Self {
        Self::from_index((self.index() + Self::ALL.len() - 1) % Self::ALL.len())
            .expect("index is always in range")
    }
}

```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib plugin::app::tests::help_tab -- --nocapture`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/app.rs
git commit -m "feat: add HelpTab (TF-585)"
```

---

### Task 3: `HelpOverlayState` and `App` wiring

**Files:**
- Modify: `src/plugin/app.rs`

**Interfaces:**
- Consumes: `HelpTab` (Task 2).
- Produces: `pub struct HelpOverlayState { pub tab: HelpTab, pub scroll: u16 }`; on `App`: `pub fn help_overlay(&self) -> Option<&HelpOverlayState>`, `pub fn open_help_overlay(&mut self)`, `pub fn close_help_overlay(&mut self)`, `pub fn help_overlay_switch_tab_forward(&mut self)`, `pub fn help_overlay_switch_tab_back(&mut self)`, `pub fn help_overlay_jump_tab(&mut self, tab: HelpTab)`, `pub fn help_overlay_scroll_down(&mut self)`, `pub fn help_overlay_scroll_up(&mut self)`. Consumed by Task 4 (`handle_help_overlay_key`) and Task 10 (`ui::draw`).

- [ ] **Step 1: Write the failing tests**

Add to the end of `src/plugin/app.rs`'s `mod tests` block:

```rust
    #[test]
    fn app_starts_with_the_help_overlay_closed() {
        let app = App::new();
        assert_eq!(app.help_overlay(), None);
    }

    #[test]
    fn open_help_overlay_starts_on_whats_new_with_scroll_at_zero() {
        let mut app = App::new();

        app.open_help_overlay();

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState { tab: HelpTab::WhatsNew, scroll: 0 })
        );
    }

    #[test]
    fn close_help_overlay_clears_it() {
        let mut app = App::new();
        app.open_help_overlay();

        app.close_help_overlay();

        assert_eq!(app.help_overlay(), None);
    }

    #[test]
    fn switch_tab_forward_cycles_and_resets_scroll() {
        let mut app = App::new();
        app.open_help_overlay();
        app.help_overlay_scroll_down();

        app.help_overlay_switch_tab_forward();

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState { tab: HelpTab::Keybindings, scroll: 0 })
        );
    }

    #[test]
    fn switch_tab_back_cycles_and_resets_scroll() {
        let mut app = App::new();
        app.open_help_overlay();

        app.help_overlay_switch_tab_back();

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState { tab: HelpTab::About, scroll: 0 })
        );
    }

    #[test]
    fn jump_tab_sets_the_tab_directly_and_resets_scroll() {
        let mut app = App::new();
        app.open_help_overlay();
        app.help_overlay_scroll_down();

        app.help_overlay_jump_tab(HelpTab::Settings);

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState { tab: HelpTab::Settings, scroll: 0 })
        );
    }

    #[test]
    fn scroll_down_then_up_returns_to_zero_and_does_not_go_negative() {
        let mut app = App::new();
        app.open_help_overlay();

        app.help_overlay_scroll_up(); // already at 0 — must not underflow/panic
        assert_eq!(app.help_overlay().unwrap().scroll, 0);

        app.help_overlay_scroll_down();
        app.help_overlay_scroll_down();
        assert_eq!(app.help_overlay().unwrap().scroll, 2);

        app.help_overlay_scroll_up();
        assert_eq!(app.help_overlay().unwrap().scroll, 1);
    }

    #[test]
    fn help_overlay_mutators_are_no_ops_when_closed() {
        let mut app = App::new();

        app.help_overlay_switch_tab_forward();
        app.help_overlay_switch_tab_back();
        app.help_overlay_jump_tab(HelpTab::About);
        app.help_overlay_scroll_down();
        app.help_overlay_scroll_up();
        app.close_help_overlay();

        assert_eq!(app.help_overlay(), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib plugin::app::tests -- --nocapture`
Expected: FAIL to compile — `no method named `help_overlay` found for struct `App``.

- [ ] **Step 3: Implement `HelpOverlayState` and the `App` methods**

Insert immediately after `HelpTab`'s closing `impl` brace from Task 2 (still before `pub struct App`):

```rust
/// State of the in-app help overlay (`?` — TF-585) while open. `None` on [`App`] means
/// closed — see [`App::help_overlay`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HelpOverlayState {
    /// The currently active tab.
    pub tab: HelpTab,
    /// Vertical scroll offset into the active tab's content, in lines. Reset to `0` on
    /// every tab switch — each tab's content is independent, so a scroll position from
    /// one tab is meaningless on another.
    pub scroll: u16,
}

```

In `App`'s struct definition, add the new field:

```rust
pub struct App {
    /// The current screen.
    screen: Screen,
    /// The current status banner, if any. See [`Status`].
    status: Option<Status>,
    /// The help overlay's state, if open (`?` — TF-585). A pure rendering layer over
    /// `screen`, never `Screen` itself — see [`Self::open_help_overlay`].
    help_overlay: Option<HelpOverlayState>,
}
```

In `App::new()`, add the new field to the struct literal:

```rust
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
            status: None,
            help_overlay: None,
        }
    }
```

At the end of `impl App { ... }`, immediately before its closing brace (i.e. right after `clear_status`), add:

```rust
    /// The help overlay's state, if open. `None` means closed — the underlying screen
    /// renders exactly as it would without this feature.
    pub fn help_overlay(&self) -> Option<&HelpOverlayState> {
        self.help_overlay.as_ref()
    }

    /// Opens the help overlay on its default tab (`WhatsNew`) with scroll reset to the
    /// top. Reopening after a close always starts fresh — no previous tab/scroll is
    /// remembered (not requested by TF-585's acceptance criteria).
    pub fn open_help_overlay(&mut self) {
        self.help_overlay = Some(HelpOverlayState::default());
    }

    /// Closes the help overlay, restoring the underlying screen exactly as it was — the
    /// overlay is a pure rendering layer over `screen`, never `Screen` itself, so
    /// there's nothing else to restore.
    pub fn close_help_overlay(&mut self) {
        self.help_overlay = None;
    }

    /// Switches to the next tab (`Tab`/`→`), wrapping from `About` back to `WhatsNew`,
    /// and resets scroll to the top. No-op if the overlay is closed.
    pub fn help_overlay_switch_tab_forward(&mut self) {
        if let Some(state) = &mut self.help_overlay {
            state.tab = state.tab.next();
            state.scroll = 0;
        }
    }

    /// Switches to the previous tab (`←`), wrapping from `WhatsNew` back to `About`, and
    /// resets scroll to the top. No-op if the overlay is closed.
    pub fn help_overlay_switch_tab_back(&mut self) {
        if let Some(state) = &mut self.help_overlay {
            state.tab = state.tab.prev();
            state.scroll = 0;
        }
    }

    /// Jumps directly to `tab` (bound to `1`-`4`) and resets scroll to the top. No-op if
    /// the overlay is closed.
    pub fn help_overlay_jump_tab(&mut self, tab: HelpTab) {
        if let Some(state) = &mut self.help_overlay {
            state.tab = tab;
            state.scroll = 0;
        }
    }

    /// Scrolls the current tab's content down one line (`j`/`↓`). No-op if the overlay
    /// is closed. Unbounded at the bottom end — `ratatui::widgets::Paragraph::scroll`
    /// clips gracefully past the end of its content, so there's no need to know each
    /// tab's exact line count here just to clamp against it.
    pub fn help_overlay_scroll_down(&mut self) {
        if let Some(state) = &mut self.help_overlay {
            state.scroll = state.scroll.saturating_add(1);
        }
    }

    /// Scrolls the current tab's content up one line (`k`/`↑`), clamped at the top.
    /// No-op if the overlay is closed.
    pub fn help_overlay_scroll_up(&mut self) {
        if let Some(state) = &mut self.help_overlay {
            state.scroll = state.scroll.saturating_sub(1);
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib plugin::app::tests -- --nocapture`
Expected: PASS, all `app.rs` tests (existing + 8 new).

- [ ] **Step 5: Commit**

```bash
git add src/plugin/app.rs
git commit -m "feat: add HelpOverlayState and App accessors (TF-585)"
```

---

### Task 4: `handle_key` wiring

**Files:**
- Modify: `src/plugin/app.rs`

**Interfaces:**
- Consumes: `HelpTab`, `HelpOverlayState`/`App` methods (Tasks 2-3).
- Produces: private `fn handle_help_overlay_key(app: &mut App, key: crossterm::event::KeyCode)`. `handle_key` itself gains two early branches; its public signature is unchanged.

- [ ] **Step 1: Write the failing tests**

Add to the end of `src/plugin/app.rs`'s `mod tests` block:

```rust
    #[test]
    fn question_mark_opens_the_overlay_from_the_menu() {
        let mut app = App::new();

        let action = handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::WhatsNew);
    }

    #[test]
    fn question_mark_opens_the_overlay_from_a_loaded_view() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert!(app.help_overlay().is_some());
    }

    #[test]
    fn question_mark_while_filtering_types_into_the_query_instead_of_opening_the_overlay() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert!(app.help_overlay().is_none());
        assert!(app.is_filtering());
    }

    #[test]
    fn overlay_owns_input_and_menu_navigation_does_not_leak_through_while_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        // `Down` is a menu-navigation key outside the overlay — while the overlay is
        // open it must not move the (invisible) menu selection.
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);

        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
        assert!(app.help_overlay().is_some());
    }

    #[test]
    fn tab_key_switches_tabs_forward_while_the_overlay_is_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);

        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::Keybindings);
    }

    #[test]
    fn right_arrow_switches_tabs_forward_while_the_overlay_is_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Right, KeyModifiers::NONE);

        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::Keybindings);
    }

    #[test]
    fn left_arrow_switches_tabs_backward_while_the_overlay_is_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Left, KeyModifiers::NONE);

        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::About);
    }

    #[test]
    fn number_keys_jump_directly_to_the_matching_tab() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::Settings);

        handle_key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::WhatsNew);
    }

    #[test]
    fn j_and_k_scroll_while_the_overlay_is_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().scroll, 2);

        handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().scroll, 1);
    }

    #[test]
    fn esc_closes_the_overlay() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(app.help_overlay().is_none());
    }

    #[test]
    fn q_closes_the_overlay() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);

        assert!(app.help_overlay().is_none());
    }

    #[test]
    fn question_mark_again_closes_the_overlay() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert!(app.help_overlay().is_none());
    }

    #[test]
    fn closing_the_overlay_leaves_the_underlying_screen_untouched() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down(); // select ENG-2
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        assert_eq!(app.current_view(), Some(ViewKind::MyIssues));
    }

    #[test]
    fn ctrl_c_quits_even_while_the_overlay_is_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib plugin::app::tests -- --nocapture`
Expected: FAIL — every new test above fails (overlay never opens; `?`/`Tab`/`Esc`/etc. currently fall through to `_ => None` or their pre-existing bindings).

- [ ] **Step 3: Implement the `handle_key` wiring**

In `src/plugin/app.rs`, change `handle_key`'s body: insert two new checks immediately after the existing `Ctrl+C` check and before `let in_menu = ...`:

```rust
    if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('c') {
        return Some(Action::Quit);
    }

    if app.help_overlay().is_some() {
        handle_help_overlay_key(app, key);
        return None;
    }

    if key == KeyCode::Char('?') && !app.is_filtering() {
        app.open_help_overlay();
        return None;
    }

    let in_menu = matches!(app.screen, Screen::Menu { .. });
```

Add the new dispatch function immediately after `handle_key`'s closing brace, before `open_config_action`:

```rust
/// Dispatches a key press while the help overlay (`?`) is open. The overlay owns all
/// input while active — see `handle_key`'s early check above — so every key reaching
/// this function is either one of the overlay's own bindings or has no effect; it never
/// falls through to a menu/view binding.
fn handle_help_overlay_key(app: &mut App, key: crossterm::event::KeyCode) {
    use crossterm::event::KeyCode;

    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => app.close_help_overlay(),
        KeyCode::Tab | KeyCode::Right => app.help_overlay_switch_tab_forward(),
        KeyCode::Left => app.help_overlay_switch_tab_back(),
        KeyCode::Char(c @ '1'..='4') => {
            if let Some(tab) = HelpTab::from_index(c as usize - '1' as usize) {
                app.help_overlay_jump_tab(tab);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => app.help_overlay_scroll_down(),
        KeyCode::Char('k') | KeyCode::Up => app.help_overlay_scroll_up(),
        _ => {}
    }
}

```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib plugin::app -- --nocapture`
Expected: PASS, every `app.rs` test (existing + new).

- [ ] **Step 5: Run the full suite to confirm no regressions elsewhere**

Run: `cargo test --all-features -- --nocapture`
Expected: PASS, all tests across the crate.

- [ ] **Step 6: Commit**

```bash
git add src/plugin/app.rs
git commit -m "feat: wire ? to open/close the help overlay in handle_key (TF-585)"
```

---

### Task 5: `config::resolved_summary`

**Files:**
- Modify: `src/plugin/config.rs`

**Interfaces:**
- Consumes: existing private `read_config_file`, existing `config_path_hint` (same file).
- Produces: `pub(crate) enum ConfigFileStatus { NotFound, Found, Invalid(String) }`, `pub(crate) struct ResolvedConfigSummary { pub path: String, pub status: ConfigFileStatus, pub api_key_set: bool, pub agent_command: Option<String>, pub team_id: Option<String>, pub project_overrides: BTreeMap<String, String> }`, `pub(crate) fn resolved_summary(config_dir: Option<&Path>, env_api_key: Option<&str>) -> ResolvedConfigSummary`. Consumed by Task 9 (`ui::settings_lines`).

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/config.rs`'s `mod tests` block. First widen its `use super::{...}` import list to include `resolved_summary, ConfigFileStatus`:

```rust
    use super::{
        config_path_hint, resolve_agent_command_override, resolve_api_key, resolved_summary,
        resolve_project_id_override, resolve_team_id_override, ConfigFileStatus,
    };
```

Then append:

```rust
    #[test]
    fn resolved_summary_reports_not_found_when_config_dir_is_unknown() {
        let summary = resolved_summary(None, None);

        assert_eq!(summary.status, ConfigFileStatus::NotFound);
        assert!(!summary.api_key_set);
        assert_eq!(summary.agent_command, None);
        assert_eq!(summary.team_id, None);
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
            "api_key = \"lin_api_x\"\nagent_command = \"my-agent\"\nteam_id = \"team-123\"\n\
             [project_overrides]\n\"herdr-linear\" = \"proj-1\"\n",
        )
        .unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        assert_eq!(summary.status, ConfigFileStatus::Found);
        assert!(summary.api_key_set);
        assert_eq!(summary.agent_command, Some("my-agent".to_string()));
        assert_eq!(summary.team_id, Some("team-123".to_string()));
        assert_eq!(
            summary.project_overrides.get("herdr-linear"),
            Some(&"proj-1".to_string())
        );
    }

    #[test]
    fn resolved_summary_found_with_no_file_api_key_still_reports_set_from_env() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "agent_command = \"my-agent\"\n").unwrap();

        let summary = resolved_summary(Some(dir.path()), Some("lin_api_from_env"));

        assert_eq!(summary.status, ConfigFileStatus::Found);
        assert!(summary.api_key_set);
    }

    #[test]
    fn resolved_summary_found_with_neither_api_key_source_reports_not_set() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "agent_command = \"my-agent\"\n").unwrap();

        let summary = resolved_summary(Some(dir.path()), None);

        assert!(!summary.api_key_set);
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib plugin::config::tests -- --nocapture`
Expected: FAIL to compile — `unresolved import `super::resolved_summary``.

- [ ] **Step 3: Implement `resolved_summary`**

Insert into `src/plugin/config.rs`, immediately after `resolve_team_id_override` and before `/// Resolve the Linear API key from the real environment...` (`load`'s doc comment):

```rust
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
    /// message is the underlying [`Error`]'s `Display` text, already user-facing.
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
    pub project_overrides: BTreeMap<String, String>,
}

/// Builds a [`ResolvedConfigSummary`] for the help overlay's Settings tab (TF-585).
/// Reads `config_dir` exactly once via [`read_config_file`] and derives every field from
/// that single `Result`, rather than composing
/// `resolve_api_key`/`resolve_agent_command_override`/`resolve_team_id_override` (each
/// of which independently re-reads the file and propagates its `Err` via `?`) — that
/// would mean either showing the same "invalid TOML" message three times over, or three
/// separately-worded failures for what is really one root cause. Pure function — callers
/// own reading the real environment, same pattern as every other `resolve_*` function in
/// this module.
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
            project_overrides: BTreeMap::new(),
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
                project_overrides: file.project_overrides,
            }
        }
        Err(e) => ResolvedConfigSummary {
            path,
            status: ConfigFileStatus::Invalid(e.to_string()),
            api_key_set: false,
            agent_command: None,
            team_id: None,
            project_overrides: BTreeMap::new(),
        },
    }
}

```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib plugin::config::tests -- --nocapture`
Expected: PASS, all `config.rs` tests (existing + 6 new).

- [ ] **Step 5: Commit**

```bash
git add src/plugin/config.rs
git commit -m "feat: add config::resolved_summary for the Settings tab (TF-585)"
```

---

### Task 6: `ui::about_lines`

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Produces: private `fn about_lines() -> Vec<String>`. Consumed by Task 10 (`draw_help_overlay`).

- [ ] **Step 1: Write the failing test**

Add to the end of `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn about_lines_include_version_repo_and_license() {
        let text = about_lines().join("\n");

        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("github.com/talent-factory/herdr-linear"));
        assert!(text.contains("MIT"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib plugin::ui::tests::about_lines -- --nocapture`
Expected: FAIL to compile — `cannot find function `about_lines` in this scope`.

- [ ] **Step 3: Implement `about_lines`**

Add to `src/plugin/ui.rs`, immediately before the `#[cfg(test)]` line at the end of the file:

```rust
/// The About tab's content (TF-585): plugin name, version, description, repo, license —
/// all resolved at compile time from `Cargo.toml` via `CARGO_PKG_*` env vars, so there's
/// nothing to keep in sync by hand when either changes.
fn about_lines() -> Vec<String> {
    vec![
        format!("herdr-linear v{}", env!("CARGO_PKG_VERSION")),
        String::new(),
        env!("CARGO_PKG_DESCRIPTION").to_string(),
        String::new(),
        format!("Repository: {}", env!("CARGO_PKG_REPOSITORY")),
        format!("License: {}", env!("CARGO_PKG_LICENSE")),
    ]
}

```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib plugin::ui::tests::about_lines -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: add the help overlay's About tab content (TF-585)"
```

---

### Task 7: `ui::keybindings_lines`

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: `crate::plugin::keybindings::KEYBINDINGS` (Task 1).
- Produces: private `fn keybindings_lines() -> Vec<String>`. Consumed by Task 10.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn keybindings_lines_include_every_binding_from_the_registry() {
        let text = keybindings_lines().join("\n");

        for binding in crate::plugin::keybindings::KEYBINDINGS {
            assert!(
                text.contains(binding.keys),
                "missing key `{}` in keybindings tab",
                binding.keys
            );
            assert!(
                text.contains(binding.action),
                "missing action `{}` in keybindings tab",
                binding.action
            );
        }
    }

    #[test]
    fn keybindings_lines_group_by_context_with_headings() {
        let lines = keybindings_lines();

        assert!(lines.contains(&"Menu:".to_string()));
        assert!(lines.contains(&"Global:".to_string()));
    }

    #[test]
    fn keybindings_lines_does_not_repeat_a_context_heading() {
        let lines = keybindings_lines();
        let heading_count = lines.iter().filter(|line| *line == "Menu:").count();

        assert_eq!(heading_count, 1);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib plugin::ui::tests::keybindings_lines -- --nocapture`
Expected: FAIL to compile — `cannot find function `keybindings_lines` in this scope`.

- [ ] **Step 3: Implement `keybindings_lines`**

Add to `src/plugin/ui.rs`, immediately after `about_lines`:

```rust
/// The Keybindings tab's content (TF-585): every entry in `keybindings::KEYBINDINGS`
/// (the single source of truth — see that module's doc comment), grouped under a
/// heading each time `context` changes. Relies on `KEYBINDINGS` grouping same-context
/// entries contiguously (an invariant that table's own tests guard) rather than
/// re-sorting, so the table's declared order (Menu, View, Filtering, Error screen,
/// Global) is what's shown, not an alphabetized one.
fn keybindings_lines() -> Vec<String> {
    let mut lines = Vec::new();
    let mut last_context: Option<&str> = None;

    for binding in crate::plugin::keybindings::KEYBINDINGS {
        if last_context != Some(binding.context) {
            if last_context.is_some() {
                lines.push(String::new());
            }
            lines.push(format!("{}:", binding.context));
            last_context = Some(binding.context);
        }
        lines.push(format!("  {:<10} {}", binding.keys, binding.action));
    }

    lines
}

```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib plugin::ui::tests::keybindings_lines -- --nocapture`
Expected: PASS, all 3 new tests.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: add the help overlay's Keybindings tab content (TF-585)"
```

---

### Task 8: `ui::whats_new_lines` and `CHANGELOG.md` backfill

**Files:**
- Modify: `src/plugin/ui.rs`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces: private `fn extract_section_after(text: &str, heading: &str) -> Option<Vec<String>>`, private `fn whats_new_lines_from(changelog: &str) -> Vec<String>`, private `fn whats_new_lines() -> Vec<String>`. Consumed by Task 10.

- [ ] **Step 1: Write the failing tests for `extract_section_after`**

Add to `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn extract_section_after_returns_entries_between_heading_and_next_section() {
        let changelog = "## [Unreleased]\n### Added\n- Thing one\n- Thing two\n\n\
                          ## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        let section = extract_section_after(changelog, "## [Unreleased]").unwrap();

        assert_eq!(
            section,
            vec![
                "### Added".to_string(),
                "- Thing one".to_string(),
                "- Thing two".to_string(),
            ]
        );
    }

    #[test]
    fn extract_section_after_stops_at_the_next_level_two_heading() {
        let changelog = "## [Unreleased]\n### Added\n- Thing\n\
                          ## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        let section = extract_section_after(changelog, "## [Unreleased]").unwrap();

        assert!(!section.iter().any(|line| line.contains("Old thing")));
    }

    #[test]
    fn extract_section_after_returns_none_for_an_empty_section() {
        let changelog = "## [Unreleased]\n\n## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        assert_eq!(extract_section_after(changelog, "## [Unreleased]"), None);
    }

    #[test]
    fn extract_section_after_returns_none_when_heading_is_missing() {
        let changelog = "## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        assert_eq!(extract_section_after(changelog, "## [Unreleased]"), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib plugin::ui::tests::extract_section_after -- --nocapture`
Expected: FAIL to compile — `cannot find function `extract_section_after``.

- [ ] **Step 3: Implement `extract_section_after`**

Add to `src/plugin/ui.rs`, immediately after `keybindings_lines`:

```rust
/// Everything between `heading` (matched verbatim, must be a full `## ...` heading line
/// present in `text`) and the next `## ` heading (or end of `text`), with leading/
/// trailing blank lines trimmed off (interior blank lines between entries are kept).
/// `None` if `heading` isn't found, or if the section is empty after trimming — both
/// read as "nothing here" to callers (see `whats_new_lines_from`, which falls back to
/// the next section in either case).
fn extract_section_after(text: &str, heading: &str) -> Option<Vec<String>> {
    let start = text.find(heading)?;
    let after_heading = &text[start + heading.len()..];
    let end = after_heading.find("\n## ").unwrap_or(after_heading.len());
    let body: Vec<&str> = after_heading[..end].lines().collect();

    let first_non_blank = body.iter().position(|line| !line.trim().is_empty())?;
    let last_non_blank = body.iter().rposition(|line| !line.trim().is_empty())?;
    Some(
        body[first_non_blank..=last_non_blank]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )
}

```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib plugin::ui::tests::extract_section_after -- --nocapture`
Expected: PASS, all 4 tests.

- [ ] **Step 5: Write the failing tests for `whats_new_lines_from`/`whats_new_lines`**

Add to `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn whats_new_lines_from_uses_unreleased_entries_when_present() {
        let changelog = "## [Unreleased]\n### Added\n- Thing one\n\n\
                          ## [0.1.0] - 2026-08-04\n### Added\n- Old\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines[0].contains(env!("CARGO_PKG_VERSION")));
        assert!(lines.contains(&"### Added".to_string()));
        assert!(lines.contains(&"- Thing one".to_string()));
        assert!(!lines.iter().any(|line| line.contains("- Old")));
    }

    #[test]
    fn whats_new_lines_from_falls_back_to_the_next_release_when_unreleased_is_empty() {
        let changelog = "## [Unreleased]\n\n## [0.1.0] - 2026-08-04\n### Added\n- First release\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines.contains(&"- First release".to_string()));
    }

    #[test]
    fn whats_new_lines_from_falls_back_when_unreleased_heading_is_missing_entirely() {
        let changelog = "## [0.1.0] - 2026-08-04\n### Added\n- First release\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines.contains(&"- First release".to_string()));
    }

    #[test]
    fn whats_new_lines_from_reports_when_nothing_parses_at_all() {
        let changelog = "Not a changelog at all.";

        let lines = whats_new_lines_from(changelog);

        assert!(lines
            .iter()
            .any(|line| line.contains("Couldn't find recent changes")));
    }

    #[test]
    fn whats_new_lines_reads_the_real_changelog_and_includes_the_current_version() {
        let lines = whats_new_lines();

        assert!(lines[0].contains(env!("CARGO_PKG_VERSION")));
        assert!(lines.len() > 2, "expected real entries, got: {lines:?}");
    }
```

- [ ] **Step 6: Run the tests to verify they fail**

Run: `cargo test --lib plugin::ui::tests::whats_new -- --nocapture`
Expected: FAIL to compile — `cannot find function `whats_new_lines_from``.

- [ ] **Step 7: Implement `whats_new_lines_from` and `whats_new_lines`**

Add to `src/plugin/ui.rs`, immediately after `extract_section_after`:

```rust
/// The What's New tab's content (TF-585): the current version as a heading, followed by
/// `CHANGELOG.md`'s `[Unreleased]` entries — embedded at compile time via `include_str!`
/// so the plugin binary never depends on `CHANGELOG.md` being present at runtime (it
/// isn't; nothing ships the source repo alongside the built binary).
fn whats_new_lines() -> Vec<String> {
    whats_new_lines_from(include_str!("../../CHANGELOG.md"))
}

/// Pure half of [`whats_new_lines`], taking the changelog content as a parameter so it's
/// testable against fixture strings without depending on the real `CHANGELOG.md` —
/// mirrors `config.rs`'s `resolve_api_key`/`load` split (pure core + thin
/// compile-time-data wrapper).
fn whats_new_lines_from(changelog: &str) -> Vec<String> {
    const UNRELEASED_HEADING: &str = "## [Unreleased]";
    let mut lines = vec![
        format!("v{} (unreleased)", env!("CARGO_PKG_VERSION")),
        String::new(),
    ];

    let section = extract_section_after(changelog, UNRELEASED_HEADING).or_else(|| {
        // `[Unreleased]` is missing or empty — fall back to the next `## [` heading
        // after it (the newest real release). Search from just past `[Unreleased]`'s
        // own heading line so this can't just re-find the same empty section; if
        // `[Unreleased]` isn't present at all, search the whole file.
        let search_from = changelog
            .find(UNRELEASED_HEADING)
            .and_then(|i| changelog[i..].find('\n').map(|nl| i + nl + 1))
            .unwrap_or(0);
        let rest = &changelog[search_from..];
        let heading_line_start = rest.find("## [")?;
        let heading_line_end = rest[heading_line_start..]
            .find('\n')
            .map(|nl| heading_line_start + nl)
            .unwrap_or(rest.len());
        extract_section_after(
            &rest[heading_line_start..],
            &rest[heading_line_start..heading_line_end],
        )
    });

    match section {
        Some(entries) => lines.extend(entries),
        None => lines.push("Couldn't find recent changes in CHANGELOG.md.".to_string()),
    }

    lines
}

```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test --lib plugin::ui::tests -- --nocapture`
Expected: PASS, all `ui.rs` tests including the 5 new ones from this step. (The last one, `whats_new_lines_reads_the_real_changelog_and_includes_the_current_version`, passes already at this point since the real `CHANGELOG.md`'s `[Unreleased]` section already has content — Step 9 backfills it with TF-585-relevant content, not with content at all.)

- [ ] **Step 9: Backfill `CHANGELOG.md`**

In `CHANGELOG.md`, the `## [Unreleased]` section currently only lists generic library-level entries from the initial `0.1.0` release. Add the plugin-facing entries for recent UI work that shipped without one, above the existing entries under `### Added`:

```markdown
## [Unreleased]

### Added

- In-app Help overlay (`?` key): What's New / Keybindings / Settings / About (TF-585)
- Type-to-filter the loaded issue list by title/identifier (TF-580)
- Guaranteed tab-per-issue on the Linear implement flow (TF-579)
- Unique per-issue herdr agent names + multi-select issues (TF-590)
```

placed as the first four bullets under the existing `### Added` heading in `[Unreleased]` (the generic library-level bullets already there stay, unchanged, below them).

- [ ] **Step 10: Run the full suite**

Run: `cargo test --all-features -- --nocapture`
Expected: PASS, all tests.

- [ ] **Step 11: Commit**

```bash
git add src/plugin/ui.rs CHANGELOG.md
git commit -m "feat: add the help overlay's What's New tab content (TF-585)

Also backfills CHANGELOG.md's [Unreleased] section with the recent
plugin-facing work (TF-579/580/590) it was missing, so the tab shows
something meaningful."
```

---

### Task 9: `ui::settings_lines`

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: `crate::plugin::config::{resolved_summary, ResolvedConfigSummary, ConfigFileStatus}` (Task 5).
- Produces: private `fn settings_lines() -> Vec<String>`, private `fn settings_lines_from(summary: &ResolvedConfigSummary) -> Vec<String>`. Consumed by Task 10.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn settings_lines_from_not_found_shows_defaults_message() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::NotFound,
            api_key_set: false,
            agent_command: None,
            team_id: None,
            project_overrides: std::collections::BTreeMap::new(),
        };

        let lines = settings_lines_from(&summary).join("\n");

        assert!(lines.contains("no file found, using defaults"));
        assert!(lines.contains("✗ Not set"));
        assert!(lines.contains("(default)"));
    }

    #[test]
    fn settings_lines_from_found_shows_masked_api_key_and_resolved_values() {
        let mut project_overrides = std::collections::BTreeMap::new();
        project_overrides.insert("herdr-linear".to_string(), "proj-1".to_string());
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::Found,
            api_key_set: true,
            agent_command: Some("my-agent".to_string()),
            team_id: Some("team-123".to_string()),
            project_overrides,
        };

        let lines = settings_lines_from(&summary).join("\n");

        assert!(lines.contains("Config: found"));
        assert!(lines.contains("✓ Set"));
        assert!(!lines.contains("lin_api_"));
        assert!(lines.contains("my-agent"));
        assert!(lines.contains("team-123"));
        assert!(lines.contains("herdr-linear"));
        assert!(lines.contains("proj-1"));
    }

    #[test]
    fn settings_lines_from_invalid_shows_the_error_message_and_no_stale_values() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::Invalid("not valid TOML".to_string()),
            api_key_set: false,
            agent_command: None,
            team_id: None,
            project_overrides: std::collections::BTreeMap::new(),
        };

        let lines = settings_lines_from(&summary).join("\n");

        assert!(lines.contains("is invalid"));
        assert!(lines.contains("not valid TOML"));
        assert!(lines.contains("✗ Not set"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib plugin::ui::tests::settings_lines -- --nocapture`
Expected: FAIL — `ResolvedConfigSummary`'s fields aren't visible (still private in `config.rs`'s public interface for `ui.rs` to construct) *or* `settings_lines_from` doesn't exist. It should already be `pub(crate)` from Task 5, so the actual failure is `cannot find function `settings_lines_from``.

- [ ] **Step 3: Implement `settings_lines`/`settings_lines_from`**

Add to `src/plugin/ui.rs`, immediately after `whats_new_lines_from`:

```rust
/// The Settings tab's content (TF-585): the plugin's currently-resolved `config.toml`
/// values. Reads the real environment once, via the same `HERDR_PLUGIN_CONFIG_DIR`/
/// `LINEAR_API_KEY` lookup `config::load()` uses, then hands off to
/// `config::resolved_summary` for the actual resolution logic — this function owns no
/// config-reading of its own, only formatting the result.
fn settings_lines() -> Vec<String> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);
    let env_api_key = std::env::var("LINEAR_API_KEY").ok();
    let summary =
        crate::plugin::config::resolved_summary(config_dir.as_deref(), env_api_key.as_deref());
    settings_lines_from(&summary)
}

/// Pure half of [`settings_lines`], taking an already-resolved summary so it's testable
/// without touching the real environment.
fn settings_lines_from(summary: &crate::plugin::config::ResolvedConfigSummary) -> Vec<String> {
    use crate::plugin::config::ConfigFileStatus;

    let mut lines = Vec::new();
    match &summary.status {
        ConfigFileStatus::NotFound => {
            lines.push("Config: no file found, using defaults.".to_string())
        }
        ConfigFileStatus::Found => lines.push("Config: found".to_string()),
        ConfigFileStatus::Invalid(message) => lines.push(format!(
            "Config: {} exists but is invalid — {message}",
            summary.path
        )),
    }
    lines.push(format!("Location: {}", summary.path));
    lines.push(String::new());

    let api_key_display = if summary.api_key_set { "✓ Set" } else { "✗ Not set" };
    lines.push(format!("api_key          = {api_key_display}"));

    let agent_command_display = summary.agent_command.as_deref().unwrap_or("(default)");
    lines.push(format!("agent_command    = {agent_command_display}"));

    let team_id_display = summary.team_id.as_deref().unwrap_or("Not set");
    lines.push(format!("team_id          = {team_id_display}"));

    if summary.project_overrides.is_empty() {
        lines.push("project_overrides: (none)".to_string());
    } else {
        lines.push("project_overrides:".to_string());
        for (repo, project_id) in &summary.project_overrides {
            lines.push(format!("  {repo:<15} = {project_id}"));
        }
    }

    lines
}

```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib plugin::ui::tests::settings_lines -- --nocapture`
Expected: PASS, all 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: add the help overlay's Settings tab content (TF-585)"
```

---

### Task 10: `draw_help_overlay` and wiring into `draw()`

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: `HelpTab`, `HelpOverlayState`, `App::help_overlay` (Tasks 2-3); `about_lines`, `keybindings_lines`, `whats_new_lines`, `settings_lines` (Tasks 6-9).
- Produces: private `fn draw_help_overlay(frame: &mut Frame, overlay: &HelpOverlayState)`, private `fn centered_rect(percent_width: u16, percent_height: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect`. `draw()`'s signature is unchanged; its body gains one new line.

- [ ] **Step 1: Write the failing tests**

Add to `src/plugin/ui.rs`'s `mod tests` block:

```rust
    #[test]
    fn help_overlay_renders_on_top_of_the_menu_when_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("Help:"));
        assert!(text.contains("What's New"));
        assert!(text.contains("Keybindings"));
        assert!(text.contains("Settings"));
        assert!(text.contains("About"));
    }

    #[test]
    fn help_overlay_marks_the_active_tab() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("> What's New"));
    }

    #[test]
    fn help_overlay_shows_the_footer_controls() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("close"));
    }

    #[test]
    fn help_overlay_switches_tab_content_on_number_jump() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('4'), KeyModifiers::NONE); // -> About

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("> About"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn help_overlay_closes_and_the_underlying_screen_reappears_unchanged() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        let text = rendered_text(&app);
        assert!(!text.contains("Help:"));
        assert!(text.contains("ENG-1"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib plugin::ui::tests::help_overlay -- --nocapture`
Expected: FAIL — the overlay never renders yet (`draw` doesn't call `draw_help_overlay`, which doesn't exist).

- [ ] **Step 3: Add `Clear` to the widget imports**

In `src/plugin/ui.rs`, change the top-of-file import:

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
```

to:

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
```

Also widen the `crate::plugin::app::{...}` import on the line above it to include `HelpOverlayState` and `HelpTab`:

```rust
use crate::plugin::app::{
    matching_issue_indices, App, HelpOverlayState, HelpTab, Screen, Status, ViewKind, ViewState,
    MENU_OPTIONS,
};
```

- [ ] **Step 4: Implement `draw_help_overlay` and wire it into `draw`**

Change `draw` (near the top of the file) from:

```rust
pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => draw_view(frame, *kind, view_state, app.status()),
    }
}
```

to:

```rust
pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => draw_view(frame, *kind, view_state, app.status()),
    }
    if let Some(overlay) = app.help_overlay() {
        draw_help_overlay(frame, overlay);
    }
}
```

Add, immediately after `settings_lines_from`'s closing brace:

```rust
/// Renders the help overlay (`?` — TF-585) on top of whatever `draw` already drew for
/// the current screen: `Clear` the area first (ratatui doesn't blank a widget's
/// background on its own — without this, stale content from beneath shows through
/// wherever this frame's text doesn't happen to overwrite it), then the tab bar +
/// scrollable body + footer, matching the herdr-file-viewer reference screenshot this
/// design follows.
fn draw_help_overlay(frame: &mut Frame, overlay: &HelpOverlayState) {
    let area = centered_rect(80, 90, frame.area());
    frame.render_widget(Clear, area);

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let title = HelpTab::ALL
        .iter()
        .map(|&tab| {
            if tab == overlay.tab {
                format!("> {}", tab.title())
            } else {
                tab.title().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("   ");

    let content = match overlay.tab {
        HelpTab::WhatsNew => whats_new_lines(),
        HelpTab::Keybindings => keybindings_lines(),
        HelpTab::Settings => settings_lines(),
        HelpTab::About => about_lines(),
    }
    .join("\n");

    let body = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Help: {title}")),
        )
        .wrap(Wrap { trim: false })
        .scroll((overlay.scroll, 0));
    frame.render_widget(body, outer[0]);

    let footer = Paragraph::new("Tab/←→ switch · 1-4 jump · j/k scroll · Esc/q/? close")
        .style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(footer, outer[1]);
}

/// A `Rect` centered within `area`, `percent_width`/`percent_height` of its size — the
/// standard ratatui popup-centering recipe (two nested percentage-based `Layout` splits,
/// taking the middle cell of each).
fn centered_rect(
    percent_width: u16,
    percent_height: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_height) / 2),
            Constraint::Percentage(percent_height),
            Constraint::Percentage((100 - percent_height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_width) / 2),
            Constraint::Percentage(percent_width),
            Constraint::Percentage((100 - percent_width) / 2),
        ])
        .split(vertical[1])[1]
}

```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib plugin::ui::tests -- --nocapture`
Expected: PASS, every `ui.rs` test (existing + all new from Tasks 6-10).

- [ ] **Step 6: Run the full suite and the quality gate**

Run: `just check` (fmt + clippy + test)
Expected: `✅ All checks passed!`

- [ ] **Step 7: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: render the help overlay on top of the current screen (TF-585)"
```

---

### Task 11: README

**Files:**
- Modify: `README.md`

**Interfaces:** None — documentation only.

- [ ] **Step 1: Add the `?` binding to the "Use" section**

In `README.md`'s `### Use` section, find this sentence (the last one in that paragraph, right before the `> [!NOTE]` block):

```
`c` to open `config.toml` from an error screen (see "Configure" above — creates the file
if it doesn't exist yet), and `Esc` to return to the menu (or, while filtering, to cancel
the filter first). Press `q` to quit the panel from anywhere (menu or view). Pressing the
key again focuses the panel if it's open elsewhere, or closes it if it's already focused.
```

Replace it with:

```
`c` to open `config.toml` from an error screen (see "Configure" above — creates the file
if it doesn't exist yet), and `Esc` to return to the menu (or, while filtering, to cancel
the filter first). Press `q` to quit the panel from anywhere (menu or view). Press `?`
from anywhere to open an in-app help overlay — **What's New** (recent changes),
**Keybindings** (every binding above, plus this one), **Settings** (your currently
resolved `config.toml` values — the API key is shown only as set/not-set, never in the
clear), and **About** (version, repo, license) — without leaving the terminal. Switch
tabs with `Tab`/`←`/`→` or `1`-`4`, scroll with `j`/`k` or the arrow keys, and close with
`Esc`, `q`, or `?` again. Pressing the key again focuses the panel if it's open
elsewhere, or closes it if it's already focused.
```

- [ ] **Step 2: Verify the change**

Run: `grep -n "help overlay" README.md`
Expected: one match, in the sentence just added.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document the ? help overlay in the Use section (TF-585)"
```

---

## Final Verification

- [ ] Run `just check` once more from a clean `git status` (nothing uncommitted) to confirm the whole feature builds, lints, and tests clean end to end.
- [ ] Manually smoke-test per the design's reference: `just plugin-reinstall` (or `cargo build --release --features plugin` + `herdr plugin link .`), open the panel, press `?` from the menu and from a loaded view, cycle all 4 tabs via `Tab`/arrows/number keys, scroll a tab with `j`/`k`, close with `Esc`/`q`/`?`, confirm `/` filtering still captures a literal `?` character instead of opening the overlay.
- [ ] Every acceptance criterion in TF-585 is covered: `?` toggle + `Esc`/`q` close (Task 4), 4 tabs switchable via `Tab`/arrows/`1`-`4` (Tasks 2-4, 10), scrollable content via `j`/`k`/arrows (Tasks 3-4), Keybindings tab from a central registry not hard-duplicated (Task 1, 7), overlay is read-only and doesn't block other plugin functions (Task 3 — pure `Option` layer over `screen`, never `Screen` itself), README updated (Task 11).
