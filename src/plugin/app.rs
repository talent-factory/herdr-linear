//! TUI application state and navigation.
//!
//! Provides pure state management for the terminal UI without any rendering logic.
//! The app starts on a menu (`Screen::Menu`) listing the configured issue views —
//! unavailable ones are shown but disabled — then moves into `Screen::View` once an
//! available one is selected, tracking that view's own display state (loading,
//! loaded issues, error) and navigation within its issue list.

use crate::Issue;
use std::collections::HashSet;
use std::path::PathBuf;

/// The views selectable from the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// Issues assigned to the authenticated user.
    MyIssues,
    /// All open issues in the current project.
    ProjectIssues,
    /// All open issues in a team.
    TeamIssues,
}

impl ViewKind {
    /// The label shown for this view in the menu.
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::MyIssues => "My Issues",
            ViewKind::ProjectIssues => "Project Issues",
            ViewKind::TeamIssues => "Team Issues",
        }
    }
}

/// A single entry in the view-selection menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuOption {
    /// The view this entry selects.
    pub kind: ViewKind,
    /// Whether this entry is selectable yet.
    pub available: bool,
}

/// The menu options in display order. All three are available as of TF-579.
pub const MENU_OPTIONS: [MenuOption; 3] = [
    MenuOption {
        kind: ViewKind::MyIssues,
        available: true,
    },
    MenuOption {
        kind: ViewKind::ProjectIssues,
        available: true,
    },
    MenuOption {
        kind: ViewKind::TeamIssues,
        available: true,
    },
];

/// The state of a single view once entered from the menu.
///
/// Note: `ViewState` deliberately does NOT derive `PartialEq` because `Issue`
/// doesn't derive it either. Tests use `matches!` for state comparisons instead.
#[derive(Debug, Clone)]
pub enum ViewState {
    /// The view is loading its issues.
    Loading,
    /// Issues have been loaded successfully.
    Loaded {
        /// The list of loaded issues.
        issues: Vec<Issue>,
        /// The index of the currently selected issue within the *filtered* subset
        /// (see [`matching_issue_indices`]) — not a raw index into `issues`. With no
        /// active filter the two coincide, since every issue matches.
        selected: usize,
        /// Indices (into `issues`) marked for a multi-issue `<Enter>` (see
        /// [`App::toggle_mark`], `Action::ImplementMany`). Empty means the single-issue
        /// `<Enter>` behavior applies to `selected` instead — see `handle_key`. Independent
        /// of the active filter: marking targets `issues` directly (via `toggle_mark`'s own
        /// filtered-to-raw-index translation), so marks survive a filter change instead of
        /// being invalidated by it.
        marked: HashSet<usize>,
        /// Type-to-filter state for this view's list. See [`FilterState`].
        filter: FilterState,
    },
    /// An error occurred.
    Error {
        /// The error message.
        message: String,
    },
}

/// Type-to-filter state for a loaded view's issue list (TF-580). `/` opens editing;
/// `Enter` confirms (query stays applied, keystrokes stop being captured); `Esc` while
/// editing cancels and clears the query, restoring the full list — distinct from `Esc`
/// outside editing, which returns to the menu as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterState {
    /// True while `/` has been pressed and not yet confirmed (`Enter`) or cancelled
    /// (`Esc`) — character keys are captured into `query` instead of triggering their
    /// usual view bindings (`q`, `o`, `r`, ...) while this is set.
    pub editing: bool,
    /// The current filter text. Empty matches every issue, so a view with no filter
    /// ever applied (or one just cancelled) behaves exactly as before this feature.
    pub query: String,
}

/// Returns the indices into `issues` of every issue whose title or identifier contains
/// `query` as a case-insensitive substring, in original order. An empty `query` matches
/// everything — the no-filter case degenerates to `0..issues.len()` rather than an
/// empty result, so callers don't need a separate branch for "no filter active".
///
/// Pure and independent of `App`/`ViewState` so it's directly unit-testable, mirroring
/// the `assignee_open_filter`/`project_open_filter` pattern in `data.rs`.
pub fn matching_issue_indices(issues: &[Issue], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..issues.len()).collect();
    }
    let query = query.to_lowercase();
    issues
        .iter()
        .enumerate()
        .filter(|(_, issue)| {
            issue.title.to_lowercase().contains(&query)
                || issue.identifier.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

/// What the UI should currently display: the view-selection menu, or an entered view.
#[derive(Debug, Clone)]
pub enum Screen {
    /// The view-selection menu. `selected` indexes into [`MENU_OPTIONS`].
    Menu {
        /// The index of the currently highlighted menu option.
        selected: usize,
    },
    /// A view has been entered from the menu.
    View(ViewKind, ViewState),
}

/// A transient status banner shown under an issue list — separate from `ViewState::Error` so
/// it doesn't discard an already-loaded issue list. Set by the `Action::Implement`
/// orchestration in `main.rs` as it progresses through the implement-on-Enter flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A success/progress message, e.g. "tab opened, agent started, set to In Progress.".
    Ok(String),
    /// A failure message, rendered distinctly from `Ok` so it's clearly actionable.
    Error(String),
}

impl Status {
    /// The banner text, regardless of variant.
    pub fn text(&self) -> &str {
        match self {
            Status::Ok(text) | Status::Error(text) => text,
        }
    }

    /// True for `Status::Error`.
    pub fn is_error(&self) -> bool {
        matches!(self, Status::Error(_))
    }
}

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

/// The main application state container.
///
/// Manages transitions between the menu and views, and navigation within a loaded
/// view's issues.
pub struct App {
    /// The current screen.
    screen: Screen,
    /// The current status banner, if any. See [`Status`].
    status: Option<Status>,
    /// The help overlay's state, if open (`?` — TF-585). A pure rendering layer over
    /// `screen`, never `Screen` itself — see [`Self::open_help_overlay`].
    help_overlay: Option<HelpOverlayState>,
}

impl App {
    /// Creates a new application on the menu, first option highlighted.
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
            status: None,
            help_overlay: None,
        }
    }

    /// The screen currently being displayed.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Moves the menu selection down one position, clamped at the last option.
    /// No-op outside the menu.
    pub fn move_menu_selection_down(&mut self) {
        if let Screen::Menu { selected } = &mut self.screen {
            if *selected + 1 < MENU_OPTIONS.len() {
                *selected += 1;
            }
        }
    }

    /// Moves the menu selection up one position, clamped at the first option.
    /// No-op outside the menu.
    pub fn move_menu_selection_up(&mut self) {
        if let Screen::Menu { selected } = &mut self.screen {
            if *selected > 0 {
                *selected -= 1;
            }
        }
    }

    /// Enters the currently highlighted menu option if it's available, transitioning
    /// to `Screen::View(kind, ViewState::Loading)`. Returns `Action::EnterView` on
    /// success so the caller knows to trigger a data fetch, or `None` if the option
    /// is unavailable or the app isn't currently on the menu.
    pub fn enter_selected_menu_option(&mut self) -> Option<Action> {
        let Screen::Menu { selected } = &self.screen else {
            return None;
        };
        let option = MENU_OPTIONS.get(*selected)?;
        if !option.available {
            return None;
        }
        self.screen = Screen::View(option.kind, ViewState::Loading);
        self.clear_status();
        Some(Action::EnterView)
    }

    /// Returns to the menu, selection reset to the first option.
    pub fn return_to_menu(&mut self) {
        self.screen = Screen::Menu { selected: 0 };
        self.clear_status();
    }

    /// The kind of the currently entered view, or `None` if on the menu.
    pub fn current_view(&self) -> Option<ViewKind> {
        match &self.screen {
            Screen::View(kind, _) => Some(*kind),
            Screen::Menu { .. } => None,
        }
    }

    /// True if the current view is in an error state. False on the menu.
    fn is_view_error(&self) -> bool {
        matches!(self.screen, Screen::View(_, ViewState::Error { .. }))
    }

    /// Sets the loaded issues on the current view and resets selection to the first
    /// issue, with nothing marked and no filter applied. No-op if not currently in a view.
    pub fn set_issues(&mut self, issues: Vec<Issue>) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(
                kind,
                ViewState::Loaded {
                    issues,
                    selected: 0,
                    marked: HashSet::new(),
                    filter: FilterState::default(),
                },
            );
        }
    }

    /// Transitions the current view to an error state with the given message.
    /// No-op if not currently in a view.
    pub fn set_error(&mut self, message: String) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(kind, ViewState::Error { message });
        }
    }

    /// Transitions the current view back to its loading state, clearing any stale status
    /// banner (matching `return_to_menu`/`enter_selected_menu_option`). No-op if not
    /// currently in a view.
    pub fn retry(&mut self) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(kind, ViewState::Loading);
            self.clear_status();
        }
    }

    /// Moves the selection down one position within the filtered subset, if there are
    /// more matching issues below. No-op outside a loaded view.
    pub fn move_selection_down(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                issues,
                selected,
                filter,
                ..
            },
        ) = &mut self.screen
        {
            let matched = matching_issue_indices(issues, &filter.query).len();
            if matched > 0 && *selected + 1 < matched {
                *selected += 1;
            }
        }
    }

    /// Moves the selection up one position within the filtered subset, if there are
    /// matching issues above. No-op outside a loaded view.
    pub fn move_selection_up(&mut self) {
        if let Screen::View(_, ViewState::Loaded { selected, .. }) = &mut self.screen {
            if *selected > 0 {
                *selected -= 1;
            }
        }
    }

    /// Returns a reference to the currently selected issue — `selected` indexes the
    /// filtered subset (see [`matching_issue_indices`]), not `issues` directly — or
    /// `None` if nothing is selected (outside a loaded view, or the filter matches
    /// nothing).
    pub fn selected_issue(&self) -> Option<&Issue> {
        match &self.screen {
            Screen::View(
                _,
                ViewState::Loaded {
                    issues,
                    selected,
                    filter,
                    ..
                },
            ) => {
                let matched = matching_issue_indices(issues, &filter.query);
                matched.get(*selected).and_then(|&index| issues.get(index))
            }
            _ => None,
        }
    }

    /// Toggles whether the currently selected issue is marked, for the multi-select
    /// `<Enter>` flow (TF-590, see `Action::ImplementMany`). `selected` indexes the filtered
    /// subset the same way `selected_issue` does (see `matching_issue_indices`), so this
    /// resolves it to `issues`'s raw index before marking — marking under an active filter
    /// must mark the right issue in `issues`, not whatever raw index `selected` happens to
    /// equal. No-op outside a loaded view, on an empty list, or when the filter matches
    /// nothing.
    pub fn toggle_mark(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                issues,
                selected,
                marked,
                filter,
            },
        ) = &mut self.screen
        {
            let matched = matching_issue_indices(issues, &filter.query);
            let Some(&raw_index) = matched.get(*selected) else {
                return;
            };
            if !marked.remove(&raw_index) {
                marked.insert(raw_index);
            }
        }
    }

    /// True if the issue at `index` is currently marked. False outside a loaded view.
    pub fn is_marked(&self, index: usize) -> bool {
        match &self.screen {
            Screen::View(_, ViewState::Loaded { marked, .. }) => marked.contains(&index),
            _ => false,
        }
    }

    /// The currently marked issues, in list order (not mark order) — empty outside a loaded
    /// view or when nothing is marked.
    pub fn marked_issues(&self) -> Vec<Issue> {
        match &self.screen {
            Screen::View(_, ViewState::Loaded { issues, marked, .. }) => {
                let mut indices: Vec<&usize> = marked.iter().collect();
                indices.sort();
                indices
                    .into_iter()
                    .filter_map(|&i| issues.get(i).cloned())
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Clears all marked issues without otherwise changing the view. Used after dispatching
    /// `Action::ImplementMany` so a completed multi-select run doesn't leave stale checkboxes
    /// behind. No-op outside a loaded view.
    pub fn clear_marks(&mut self) {
        if let Screen::View(_, ViewState::Loaded { marked, .. }) = &mut self.screen {
            marked.clear();
        }
    }

    /// True while the current view's filter is being edited (`/` pressed, not yet
    /// confirmed or cancelled). False outside a loaded view.
    fn is_filtering(&self) -> bool {
        matches!(
            &self.screen,
            Screen::View(_, ViewState::Loaded { filter, .. }) if filter.editing
        )
    }

    /// Opens filter editing (bound to `/`) — an existing query, if any, stays and can be
    /// extended or erased rather than resetting, so pressing `/` again after `Enter`
    /// resumes editing the same filter. No-op outside a loaded view.
    pub fn start_filtering(&mut self) {
        if let Screen::View(_, ViewState::Loaded { filter, .. }) = &mut self.screen {
            filter.editing = true;
        }
    }

    /// Appends `c` to the filter query and resets selection to the first match — a
    /// narrower query invalidates whatever the old index pointed at. No-op unless
    /// currently editing.
    pub fn push_filter_char(&mut self, c: char) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                filter, selected, ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.query.push(c);
                *selected = 0;
            }
        }
    }

    /// Removes the last character of the filter query, if any, and resets selection.
    /// No-op unless currently editing.
    pub fn pop_filter_char(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                filter, selected, ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.query.pop();
                *selected = 0;
            }
        }
    }

    /// Confirms the current filter (bound to `Enter` while editing): stops capturing
    /// keystrokes but leaves the query — and therefore the narrowed list — applied.
    /// No-op unless currently editing.
    pub fn confirm_filter(&mut self) {
        if let Screen::View(_, ViewState::Loaded { filter, .. }) = &mut self.screen {
            filter.editing = false;
        }
    }

    /// Cancels filtering (bound to `Esc` while editing): stops capturing keystrokes and
    /// clears the query, restoring the full issue list. Resets selection since it was
    /// indexing the now-discarded filtered subset. No-op unless currently editing.
    pub fn cancel_filter(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                filter, selected, ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.editing = false;
                filter.query.clear();
                *selected = 0;
            }
        }
    }

    /// The current status banner, if any.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    /// Sets the status banner, replacing any existing one.
    pub fn set_status(&mut self, status: Status) {
        self.status = Some(status);
    }

    /// Clears the status banner.
    pub fn clear_status(&mut self) {
        self.status = None;
    }

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

    /// Scrolls the current tab's content down one line (`j`/`↓`), clamped so the stored
    /// offset can never advance past the last line of `content_line_count` (the active
    /// tab's rendered line count — see `ui::content_line_count`, which callers must pass
    /// since only `ui.rs` knows each tab's content). No-op if the overlay is closed.
    /// Final-review fix (TF-585): previously unbounded, on the assumption that
    /// `ratatui::widgets::Paragraph::scroll` clips gracefully past the end of its content
    /// at render time — true, but clamping only there (not here) meant a held-down `j`
    /// could drive the stored offset far past the content, leaving the panel looking
    /// blank until an equal number of `k` presses brought it back into range.
    pub fn help_overlay_scroll_down(&mut self, content_line_count: usize) {
        if let Some(state) = &mut self.help_overlay {
            let max_scroll = content_line_count.saturating_sub(1) as u16;
            state.scroll = state.scroll.saturating_add(1).min(max_scroll);
        }
    }

    /// Scrolls the current tab's content up one line (`k`/`↑`), clamped at the top.
    /// No-op if the overlay is closed.
    pub fn help_overlay_scroll_up(&mut self) {
        if let Some(state) = &mut self.help_overlay {
            state.scroll = state.scroll.saturating_sub(1);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
// `Issue` is the largest variant by far; boxing it would ripple into every call site that
// constructs or matches `Action::Implement`, and the implement-on-Enter flow (`main.rs`'s
// `start_implementation`) needs the full `Issue` — not just its id — to build the prompt and
// resolve the in-progress workflow state, so a bare `Issue` is the right shape here.
#[allow(clippy::large_enum_variant)]
pub enum Action {
    Quit,
    OpenInBrowser(String),
    Retry,
    /// A menu option was entered; the caller should trigger a data fetch for the
    /// now-current view (see [`App::current_view`]).
    EnterView,
    /// `<Enter>` was pressed on a selected issue with nothing marked: open a herdr tab,
    /// start the preferred coding agent, set the issue to "In Progress", and inject the
    /// implement prompt once ready. Orchestrated in `main.rs`'s `start_implementation`.
    Implement(Issue),
    /// `<Enter>` was pressed with one or more issues marked (TF-590, see
    /// [`App::toggle_mark`]): run the same flow as `Implement` for each marked issue,
    /// sequentially, and summarize the results in one status banner. Orchestrated in
    /// `main.rs`'s `start_implementation_many`.
    ImplementMany(Vec<Issue>),
    /// `c` was pressed on the error screen: open `config.toml` (creating the directory and
    /// a starter file first if either is missing) in the user's editor. Handled in
    /// `main.rs`'s `event_loop`, mirroring the existing `OpenInBrowser` → `open::that`
    /// pattern.
    OpenConfig(PathBuf),
}

/// Map a key press to an [`Action`], applying any state change (menu navigation,
/// entering a view, list navigation, retry, returning to the menu) directly to
/// `app`. Returns `None` when the key had no effect or only changed state in place.
///
/// `modifiers` must be threaded through from the real `KeyEvent` (see `main.rs`'s
/// `event_loop`) rather than assumed empty: raw mode delivers `Ctrl+C` as an ordinary
/// `KeyCode::Char('c')` key event, not `SIGINT`, so without checking modifiers it would be
/// indistinguishable from a plain `c` press — including on the error screen, where a plain
/// `c` now triggers `Action::OpenConfig` (filesystem writes + spawning an external opener).
/// The interrupt reflex is handled unconditionally, before any screen-specific dispatch, so
/// it always quits rather than ever being reinterpreted as a screen's own binding.
///
/// `/` opens type-to-filter on a loaded view's issue list (TF-580); once editing, this
/// function's own filtering branch takes over key dispatch entirely (see
/// `App::is_filtering`) until `Enter` confirms or `Esc` cancels, so none of the
/// ordinary view bindings below (`q`, `o`, `r`, `Enter`-to-implement, ...) fire on a
/// character typed into the filter.
pub fn handle_key(
    app: &mut App,
    key: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<Action> {
    use crossterm::event::{KeyCode, KeyModifiers};

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

    if in_menu {
        return match key {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Down => {
                app.move_menu_selection_down();
                None
            }
            KeyCode::Up => {
                app.move_menu_selection_up();
                None
            }
            KeyCode::Enter => app.enter_selected_menu_option(),
            _ => None,
        };
    }

    // While the filter is being edited, character keys are captured into the query
    // instead of dispatching their usual view bindings (`q` would otherwise quit
    // instead of typing a "q"; `Enter` would otherwise trigger `Implement` instead of
    // confirming the filter). This must come before the general view match below so it
    // takes priority. Navigation (`Up`/`Down`) still works while editing, so the list
    // narrows live as you type without losing the ability to move the highlight.
    if app.is_filtering() {
        return match key {
            KeyCode::Enter => {
                app.confirm_filter();
                None
            }
            KeyCode::Esc => {
                app.cancel_filter();
                None
            }
            KeyCode::Backspace => {
                app.pop_filter_char();
                None
            }
            KeyCode::Down => {
                app.move_selection_down();
                None
            }
            KeyCode::Up => {
                app.move_selection_up();
                None
            }
            KeyCode::Char(c) => {
                app.push_filter_char(c);
                None
            }
            _ => None,
        };
    }

    match key {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('/') => {
            app.start_filtering();
            None
        }
        KeyCode::Esc => {
            app.return_to_menu();
            None
        }
        KeyCode::Down => {
            app.move_selection_down();
            None
        }
        KeyCode::Up => {
            app.move_selection_up();
            None
        }
        KeyCode::Char('o') => app
            .selected_issue()
            .map(|issue| Action::OpenInBrowser(issue.url.clone())),
        KeyCode::Char(' ') => {
            app.toggle_mark();
            None
        }
        KeyCode::Enter => {
            let marked = app.marked_issues();
            if marked.is_empty() {
                app.selected_issue()
                    .map(|issue| Action::Implement(issue.clone()))
            } else {
                Some(Action::ImplementMany(marked))
            }
        }
        KeyCode::Char('r') => {
            if app.is_view_error() {
                app.retry();
                Some(Action::Retry)
            } else {
                None
            }
        }
        // `modifiers.is_empty()` matters beyond the `Ctrl+C` case already handled above: it
        // also keeps e.g. `Alt+C`/`Shift+C` from triggering config-file writes, since this
        // arm's side effect (create + open `config.toml`) is meant for a deliberate, bare
        // `c` press, not any key event that happens to carry the `'c'` character.
        KeyCode::Char('c') if modifiers.is_empty() && app.is_view_error() => {
            open_config_action(std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").as_deref())
        }
        _ => None,
    }
}

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
        KeyCode::Char('j') | KeyCode::Down => {
            // `help_overlay_scroll_down` needs `&mut self` to clamp against the active
            // tab's content length, so the tab (`HelpTab` is `Copy`) is read into a local
            // first rather than holding an immutable borrow of `app` across the call.
            let tab = app.help_overlay().map(|state| state.tab);
            if let Some(tab) = tab {
                app.help_overlay_scroll_down(crate::plugin::ui::content_line_count(tab));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => app.help_overlay_scroll_up(),
        _ => {}
    }
}

/// Pure half of the `c`-in-error-state key handling, split out from [`handle_key`] purely
/// so it's unit-testable without mutating the real process environment (no `App` field
/// caches the config dir — every `load_*`/`resolve_*` function in `config.rs` re-reads
/// `HERDR_PLUGIN_CONFIG_DIR` fresh on each call rather than caching it — see e.g.
/// `load_project_id_override` — so this mirrors the same pattern rather than introducing
/// new app state for a single keypress). `None` (env var unset) is a no-op, same as `r`
/// outside the error view — near-impossible in practice since herdr always sets it when
/// launching the plugin.
fn open_config_action(config_dir_env: Option<&std::ffi::OsStr>) -> Option<Action> {
    config_dir_env.map(|dir| Action::OpenConfig(PathBuf::from(dir).join("config.toml")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to create a sample issue for testing.
    fn sample_issue(identifier: &str) -> Issue {
        Issue {
            id: format!("id-{}", identifier),
            identifier: identifier.to_string(),
            title: format!("Issue {}", identifier),
            description: None,
            state: crate::IssueState {
                id: "state-id".to_string(),
                name: "In Progress".to_string(),
                r#type: "started".to_string(),
            },
            priority: 0,
            estimate: None,
            team: crate::Team {
                id: "team-id".to_string(),
                key: "ENG".to_string(),
                name: "Engineering".to_string(),
                description: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            assignee: None,
            creator: Some(crate::User {
                id: "user-id".to_string(),
                email: "test@example.com".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            }),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            started_at: None,
            completed_at: None,
            cycle: None,
            project: None,
            labels: crate::LabelConnection { nodes: vec![] },
            url: format!("https://linear.app/team/issue/{}", identifier),
        }
    }

    /// An `App` that has already entered the "My Issues" view (still `Loading`),
    /// for tests that exercise view-level behavior without re-navigating the menu
    /// each time.
    fn app_in_my_issues_view() -> App {
        let mut app = App::new();
        app.enter_selected_menu_option();
        app
    }

    #[test]
    fn app_starts_at_the_menu() {
        let app = App::new();
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn menu_selection_moves_down_and_clamps_at_the_last_option() {
        let mut app = App::new();

        app.move_menu_selection_down();
        assert!(matches!(app.screen, Screen::Menu { selected: 1 }));

        app.move_menu_selection_down();
        assert!(matches!(app.screen, Screen::Menu { selected: 2 }));

        app.move_menu_selection_down();
        assert!(matches!(app.screen, Screen::Menu { selected: 2 }));
    }

    #[test]
    fn menu_selection_moves_up_and_clamps_at_the_first_option() {
        let mut app = App::new();
        app.move_menu_selection_down();

        app.move_menu_selection_up();
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));

        app.move_menu_selection_up();
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn entering_the_available_option_transitions_to_loading_and_returns_enter_view() {
        let mut app = App::new();

        let action = app.enter_selected_menu_option();

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
        assert_eq!(app.current_view(), Some(ViewKind::MyIssues));
    }

    #[test]
    fn entering_the_project_issues_option_transitions_to_loading_and_returns_enter_view() {
        let mut app = App::new();
        app.move_menu_selection_down(); // -> Project Issues, available

        let action = app.enter_selected_menu_option();

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::ProjectIssues, ViewState::Loading)
        ));
        assert_eq!(app.current_view(), Some(ViewKind::ProjectIssues));
    }

    #[test]
    fn entering_the_team_issues_option_transitions_to_loading_and_returns_enter_view() {
        let mut app = App::new();
        app.move_menu_selection_down();
        app.move_menu_selection_down(); // -> Team Issues, available since TF-579

        let action = app.enter_selected_menu_option();

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::TeamIssues, ViewState::Loading)
        ));
        assert_eq!(app.current_view(), Some(ViewKind::TeamIssues));
    }

    #[test]
    fn menu_options_covers_every_view_kind() {
        fn assert_present(kind: ViewKind) {
            assert!(
                MENU_OPTIONS.iter().any(|option| option.kind == kind),
                "{kind:?} is missing from MENU_OPTIONS"
            );
        }

        // Exhaustive match over `ViewKind`: adding a variant without a matching
        // arm here fails to compile, forcing this test (and MENU_OPTIONS) to be
        // kept in sync.
        match ViewKind::MyIssues {
            ViewKind::MyIssues => {}
            ViewKind::ProjectIssues => {}
            ViewKind::TeamIssues => {}
        }
        assert_present(ViewKind::MyIssues);
        assert_present(ViewKind::ProjectIssues);
        assert_present(ViewKind::TeamIssues);
    }

    #[test]
    fn set_issues_transitions_to_loaded_with_first_selected() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        assert!(matches!(
            &app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loaded { .. })
        ));
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn move_selection_down_advances_through_issues() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![
            sample_issue("ENG-1"),
            sample_issue("ENG-2"),
            sample_issue("ENG-3"),
        ]);

        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");

        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-3");
    }

    #[test]
    fn move_selection_down_clamps_at_the_end() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        app.move_selection_down();
        app.move_selection_down();

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn move_selection_up_retreats_and_clamps_at_the_start() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn navigation_on_an_empty_list_does_not_panic() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        app.move_selection_down();
        app.move_selection_up();

        assert!(app.selected_issue().is_none());
    }

    #[test]
    fn set_error_moves_to_error_state() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        assert!(matches!(
            &app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Error { message }) if message == "boom"
        ));
    }

    #[test]
    fn retry_moves_back_to_loading() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        app.retry();

        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
    }

    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn down_key_moves_selection_and_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn up_key_moves_selection_and_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        let action = handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn o_key_returns_open_in_browser_with_the_selected_issue_url() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::OpenInBrowser(
                "https://linear.app/team/issue/ENG-1".to_string()
            ))
        );
    }

    #[test]
    fn o_key_on_an_empty_list_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('o'), KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn enter_key_returns_implement_action_with_the_selected_issue() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, Some(Action::Implement(sample_issue("ENG-1"))));
    }

    #[test]
    fn space_key_toggles_the_mark_on_the_selected_issue_and_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_marked(0));

        let action = handle_key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(!app.is_marked(0));
    }

    #[test]
    fn space_key_on_an_empty_list_does_not_panic() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        let action = handle_key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(!app.is_marked(0));
    }

    #[test]
    fn marked_issues_returns_marks_in_list_order_not_mark_order() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![
            sample_issue("ENG-1"),
            sample_issue("ENG-2"),
            sample_issue("ENG-3"),
        ]);

        // Mark ENG-3 first, then ENG-1 — the result must still come back list-ordered.
        app.move_selection_down();
        app.move_selection_down();
        app.toggle_mark();
        app.move_selection_up();
        app.move_selection_up();
        app.toggle_mark();

        let identifiers: Vec<String> = app
            .marked_issues()
            .iter()
            .map(|issue| issue.identifier.clone())
            .collect();
        assert_eq!(identifiers, vec!["ENG-1".to_string(), "ENG-3".to_string()]);
    }

    #[test]
    fn clear_marks_removes_every_mark() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.toggle_mark();
        app.move_selection_down();
        app.toggle_mark();

        app.clear_marks();

        assert!(app.marked_issues().is_empty());
        assert!(!app.is_marked(0));
        assert!(!app.is_marked(1));
    }

    #[test]
    fn set_issues_clears_marks_left_over_from_a_previous_list() {
        // Regression guard: a second `set_issues` call (e.g. a retry/re-fetch) must reset
        // `marked` alongside `issues`/`selected`, not just on first entry into the view —
        // otherwise a stale index could point at the wrong issue (or past the end) in the new
        // list.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.toggle_mark();
        assert!(app.is_marked(0));

        app.set_issues(vec![sample_issue("ENG-3")]);

        assert!(app.marked_issues().is_empty());
        assert!(!app.is_marked(0));
    }

    #[test]
    fn enter_key_with_marked_issues_returns_implement_many_in_list_order() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![
            sample_issue("ENG-1"),
            sample_issue("ENG-2"),
            sample_issue("ENG-3"),
        ]);
        app.move_selection_down();
        handle_key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE); // marks ENG-2
        app.move_selection_down();
        handle_key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE); // marks ENG-3

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::ImplementMany(vec![
                sample_issue("ENG-2"),
                sample_issue("ENG-3")
            ]))
        );
    }

    #[test]
    fn enter_key_with_nothing_marked_falls_back_to_the_single_issue_action() {
        // Regression guard: unmarked `<Enter>` behavior must stay exactly the pre-TF-590
        // single-issue case, regardless of which issue is currently selected.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, Some(Action::Implement(sample_issue("ENG-2"))));
    }

    #[test]
    fn enter_key_in_a_view_on_an_empty_list_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        assert_eq!(
            handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn app_starts_with_no_status() {
        let app = App::new();
        assert_eq!(app.status(), None);
    }

    #[test]
    fn set_status_stores_the_message_and_error_flag() {
        let mut app = App::new();

        app.set_status(Status::Ok("started".to_string()));
        assert_eq!(app.status(), Some(&Status::Ok("started".to_string())));

        app.set_status(Status::Error("boom".to_string()));
        assert_eq!(app.status(), Some(&Status::Error("boom".to_string())));
    }

    #[test]
    fn clear_status_removes_it() {
        let mut app = App::new();
        app.set_status(Status::Ok("started".to_string()));

        app.clear_status();

        assert_eq!(app.status(), None);
    }

    #[test]
    fn returning_to_the_menu_clears_status() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status(Status::Ok("started".to_string()));

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(app.status(), None);
    }

    #[test]
    fn entering_a_view_from_the_menu_clears_status() {
        let mut app = App::new();
        app.set_status(Status::Error("stale".to_string()));

        app.enter_selected_menu_option();

        assert_eq!(app.status(), None);
    }

    #[test]
    fn retry_clears_a_stale_status_banner() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());
        app.set_status(Status::Error("stale implement status".to_string()));

        app.retry();

        assert_eq!(app.status(), None);
    }

    #[test]
    fn q_key_from_the_menu_returns_quit() {
        let mut app = App::new();

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE),
            Some(Action::Quit)
        );
    }

    #[test]
    fn esc_key_from_the_menu_returns_quit() {
        let mut app = App::new();

        assert_eq!(
            handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE),
            Some(Action::Quit)
        );
    }

    #[test]
    fn enter_key_on_the_default_menu_selection_enters_my_issues() {
        let mut app = App::new();

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
    }

    #[test]
    fn enter_key_on_the_team_issues_menu_selection_enters_team_issues() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE); // -> Team Issues, available since TF-579

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::TeamIssues, ViewState::Loading)
        ));
    }

    #[test]
    fn up_key_from_the_menu_moves_selection_and_returns_no_action() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);

        let action = handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn q_key_from_a_view_returns_quit() {
        let mut app = app_in_my_issues_view();

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE),
            Some(Action::Quit)
        );
    }

    #[test]
    fn esc_key_from_a_loaded_view_returns_to_the_menu() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn esc_key_from_an_error_view_returns_to_the_menu() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        let action = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn reentering_a_view_after_esc_starts_a_fresh_loading_state() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
    }

    #[test]
    fn r_key_in_error_state_retries_and_returns_retry_action() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        let action = handle_key(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);

        assert_eq!(action, Some(Action::Retry));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
    }

    #[test]
    fn r_key_outside_error_state_does_nothing() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('r'), KeyModifiers::NONE),
            None
        );
    }

    // `c`-key handling is split across two layers precisely so it's testable without
    // mutating the real process environment (which, run in parallel with every other test
    // in this binary, would be a data race on shared global state): `open_config_action`
    // is a pure function taking an injected `Option<&OsStr>`, tested directly here. The one
    // `handle_key` test that needs to observe the *real* `c`-in-error-state wiring end to
    // end (`c_key_in_error_state_opens_config_when_dir_is_set`) mutates the process
    // environment deliberately and guards it with `ENV_LOCK` — see that test's comment.

    #[test]
    fn open_config_action_builds_path_when_config_dir_is_known() {
        let dir = std::ffi::OsStr::new("/fake/config/dir");

        let action = open_config_action(Some(dir));

        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
        );
    }

    #[test]
    fn open_config_action_does_nothing_when_config_dir_is_unknown() {
        assert_eq!(open_config_action(None), None);
    }

    /// Serializes tests that mutate `HERDR_PLUGIN_CONFIG_DIR` (process-global state) against
    /// each other. Currently only one test needs this, but the lock makes that an enforced
    /// property rather than an implicit one that silently stops holding the moment a second
    /// such test is added.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn c_key_in_error_state_opens_config_when_dir_is_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        // A concrete, known-non-None expectation — not a self-referential re-read of the
        // same env var `handle_key` just consulted — so a regression that mis-wires the
        // `is_view_error()` guard (or deletes the arm entirely, falling through to
        // `_ => None`) actually fails this test instead of passing vacuously.
        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
        );
    }

    #[test]
    fn c_key_outside_error_state_does_nothing() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn ctrl_c_quits_regardless_of_screen() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn ctrl_c_quits_on_error_screen_instead_of_opening_config() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn modified_c_on_error_screen_does_not_open_config() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        // Alt+C (or any other modifier combination besides the Ctrl+C handled above) must
        // not be treated as the bare `c` keybinding — it's not the deliberate press this
        // side-effecting action is meant for.
        assert_eq!(
            handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::ALT),
            None
        );
    }

    // TF-580: type-to-filter. `matching_issue_indices` is tested directly as a pure
    // function first (mirrors `assignee_open_filter`/`project_open_filter` in
    // `data.rs`), then the `App`/`handle_key` integration around it.

    #[test]
    fn matching_issue_indices_returns_every_index_in_order_for_an_empty_query() {
        let issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];

        assert_eq!(matching_issue_indices(&issues, ""), vec![0, 1]);
    }

    #[test]
    fn matching_issue_indices_matches_by_title_case_insensitively() {
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        issues[1].title = "Add dark mode".to_string();

        assert_eq!(matching_issue_indices(&issues, "LOGIN"), vec![0]);
    }

    #[test]
    fn matching_issue_indices_matches_by_identifier() {
        let issues = vec![sample_issue("ENG-1"), sample_issue("BUG-2")];

        assert_eq!(matching_issue_indices(&issues, "bug"), vec![1]);
    }

    #[test]
    fn matching_issue_indices_excludes_issues_matching_neither_field() {
        let issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];

        assert_eq!(
            matching_issue_indices(&issues, "nonexistent"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn slash_key_starts_filtering() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_filtering());
    }

    #[test]
    fn slash_key_outside_a_loaded_view_does_nothing() {
        let mut app = app_in_my_issues_view(); // still Loading, not Loaded

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);

        assert!(!app.is_filtering());
    }

    #[test]
    fn typing_while_filtering_appends_to_the_query_instead_of_triggering_its_usual_binding() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);

        // 'q' would normally quit — while filtering it must be captured as text instead.
        let action = handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_filtering());
    }

    #[test]
    fn filtering_narrows_the_list_and_resets_selection() {
        let mut app = app_in_my_issues_view();
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        issues[1].title = "Add dark mode".to_string();
        app.set_issues(issues);
        app.move_selection_down(); // select ENG-2 before narrowing

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "login".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn backspace_while_filtering_removes_the_last_character() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE); // matches nothing

        handle_key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);

        // Query is empty again — everything matches, same as no filter at all.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn enter_confirms_the_filter_and_leaves_the_narrowed_list_applied() {
        let mut app = app_in_my_issues_view();
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        app.set_issues(issues);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "login".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // Confirming must not trigger `Implement` — Enter's usual view-level meaning —
        // while still leaving the filter (and therefore the narrowed selection) applied.
        assert_eq!(action, None);
        assert!(!app.is_filtering());
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn navigation_after_confirming_a_filter_still_moves_within_the_narrowed_subset() {
        let mut app = app_in_my_issues_view();
        let mut issues = vec![
            sample_issue("ENG-1"),
            sample_issue("ENG-2"),
            sample_issue("ENG-3"),
        ];
        issues[0].title = "match one".to_string();
        issues[1].title = "no hit".to_string();
        issues[2].title = "match two".to_string();
        app.set_issues(issues);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "match".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);

        // ENG-2 ("no hit") is skipped entirely — Down moves to the next *matching* issue.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-3");
    }

    #[test]
    fn esc_while_filtering_cancels_and_restores_the_full_list() {
        let mut app = app_in_my_issues_view();
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        app.set_issues(issues);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "login".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(!app.is_filtering());
        // Back to the full, unfiltered list — Esc-to-cancel took priority over Esc's
        // usual "return to the menu" binding.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        assert_eq!(app.current_view(), Some(ViewKind::MyIssues));
    }

    #[test]
    fn esc_after_confirming_a_filter_returns_to_the_menu_as_usual() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // confirm, stop editing

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        // No longer editing, so Esc falls through to its ordinary view-level binding.
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn selected_issue_is_none_when_the_filter_matches_nothing() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "nonexistent".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue(), None);
    }

    #[test]
    fn reentering_a_view_clears_any_previous_filter() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE); // matches nothing
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE); // cancel filter
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE); // back to menu

        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // re-enter My Issues
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        assert!(!app.is_filtering());
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

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
            Some(&HelpOverlayState {
                tab: HelpTab::WhatsNew,
                scroll: 0
            })
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
        app.help_overlay_scroll_down(100); // generously large — not testing the clamp here

        app.help_overlay_switch_tab_forward();

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState {
                tab: HelpTab::Keybindings,
                scroll: 0
            })
        );
    }

    #[test]
    fn switch_tab_back_cycles_and_resets_scroll() {
        let mut app = App::new();
        app.open_help_overlay();

        app.help_overlay_switch_tab_back();

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState {
                tab: HelpTab::About,
                scroll: 0
            })
        );
    }

    #[test]
    fn jump_tab_sets_the_tab_directly_and_resets_scroll() {
        let mut app = App::new();
        app.open_help_overlay();
        app.help_overlay_scroll_down(100); // generously large — not testing the clamp here

        app.help_overlay_jump_tab(HelpTab::Settings);

        assert_eq!(
            app.help_overlay(),
            Some(&HelpOverlayState {
                tab: HelpTab::Settings,
                scroll: 0
            })
        );
    }

    #[test]
    fn scroll_down_then_up_returns_to_zero_and_does_not_go_negative() {
        let mut app = App::new();
        app.open_help_overlay();

        app.help_overlay_scroll_up(); // already at 0 — must not underflow/panic
        assert_eq!(app.help_overlay().unwrap().scroll, 0);

        app.help_overlay_scroll_down(100); // generously large — not testing the clamp here
        app.help_overlay_scroll_down(100); // generously large — not testing the clamp here
        assert_eq!(app.help_overlay().unwrap().scroll, 2);

        app.help_overlay_scroll_up();
        assert_eq!(app.help_overlay().unwrap().scroll, 1);
    }

    /// Final-review fix (TF-585): scrolling down more times than the tab has content
    /// must clamp the stored offset at `content_line_count - 1`, not run away past it —
    /// otherwise the panel looks blank until an equal number of `k` presses recover it.
    #[test]
    fn scroll_down_clamps_at_the_last_line_of_the_tabs_content() {
        let mut app = App::new();
        app.open_help_overlay();

        for _ in 0..10 {
            app.help_overlay_scroll_down(3);
        }

        assert_eq!(app.help_overlay().unwrap().scroll, 2);
    }

    #[test]
    fn help_overlay_mutators_are_no_ops_when_closed() {
        let mut app = App::new();

        app.help_overlay_switch_tab_forward();
        app.help_overlay_switch_tab_back();
        app.help_overlay_jump_tab(HelpTab::About);
        app.help_overlay_scroll_down(100); // generously large — not testing the clamp here
        app.help_overlay_scroll_up();
        app.close_help_overlay();

        assert_eq!(app.help_overlay(), None);
    }

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
}
