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
// `Loaded` carries a `CommentState`, which (since TF-648's PR #53 review fix) embeds a
// full `Issue` by value in its `Editing`/`Failed` variants — the same "boxing would ripple
// into every call site" tradeoff `Action`'s own `#[allow(clippy::large_enum_variant)]`
// already accepts for the exact same reason, and for consistency with it.
#[allow(clippy::large_enum_variant)]
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
        /// Comment-composition state for the selected issue. See [`CommentState`].
        comment: CommentState,
        /// Vertical scroll offset into the Detail pane's rendered lines for the
        /// currently selected issue, in rows. Reset to `0` whenever `selected` changes
        /// (a new issue's description has nothing to do with the old scroll position) —
        /// see `move_selection_down`/`move_selection_up` and the filter mutators that
        /// also reset `selected`. Mirrors `HelpOverlayState::scroll`'s same
        /// reset-on-context-switch discipline.
        detail_scroll: u16,
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

/// Comment-composition state for a loaded view's issue list (TF-648). A dedicated key
/// (`m`) opens editing on the currently selected issue; `Enter` confirms — handing the
/// draft to [`Action::AddComment`] for `main.rs`'s `event_loop` to actually submit via
/// `client.add_comment()` — and `Esc` cancels without sending, the same interaction shape
/// as [`FilterState`]. Unlike [`FilterState`], `Up`/`Down` (and the mouse wheel — see
/// `handle_mouse`) are *not* passed through while editing.
///
/// Modeled as a small state machine rather than `FilterState`'s independent
/// flag-plus-buffer shape, because the two fields here *aren't* independent: `body` is
/// only ever meaningful while a draft is open, so `{editing: false, body: "stale"}` would
/// be a representable-but-illegal state under that shape (nothing enforces the pairing
/// beyond every mutator happening to clear both together). `Idle`/`Editing`/`Failed` make
/// the illegal combination unrepresentable instead of merely avoided by convention.
///
/// `Editing` additionally captures its target `issue` once, at [`App::start_commenting`]
/// time, rather than leaving [`App::confirm_comment`] to re-resolve "whichever issue is
/// selected" when the draft is confirmed. That's a second, independent defense for the
/// same goal `Up`/`Down` exclusion serves: a still-open draft must never end up posted to
/// a different issue than the one shown when composition began — captured once, that's
/// true no matter what moves the selection underneath it later (PR #53 review: the mouse
/// wheel used to be able to, since `Up`/`Down` exclusion alone didn't cover it).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum CommentState {
    /// No draft in progress, and nothing to resume.
    #[default]
    Idle,
    /// `m` has been pressed and not yet confirmed (`Enter`) or cancelled (`Esc`) —
    /// character keys are captured into `body` instead of triggering their usual view
    /// bindings while this variant is active.
    Editing {
        /// The issue this draft will be attached to when confirmed — fixed for the
        /// draft's lifetime, never re-read from `selected` at confirm time.
        issue: Issue,
        /// The comment text composed so far.
        body: String,
    },
    /// A confirmed draft that failed to send — network error, Linear API error, or (in
    /// principle; see `main.rs`'s `event_loop`) no client connected yet. Resumable: if
    /// `m` is pressed again while the *same* issue is still selected, `start_commenting`
    /// restores `body` here instead of starting blank, so a transient failure doesn't
    /// mean retyping the whole comment (PR #53 review — previously the draft was simply
    /// discarded on any send failure). Deliberately not the state shown by the draft
    /// banner (`plugin::ui::draw`'s `CommentState::Editing` match arm) — staying out of
    /// `Editing` here means the send failure's `Status::Error` banner stays visible
    /// instead of being immediately overwritten by a reopened draft.
    Failed {
        /// The issue the failed draft was for. Only resumed by `start_commenting` when
        /// this still matches the currently selected issue.
        issue: Issue,
        /// The comment text that failed to send.
        body: String,
    },
}

/// Returns the indices into `issues` matching `query`, in the order they should be
/// displayed/navigated. An empty `query` matches everything, in `issues`' original order
/// — the no-filter case degenerates to `0..issues.len()` rather than an empty result, so
/// callers don't need a separate branch for "no filter active".
///
/// `query` is parsed through the same DSL `crate::plugin::query::parse_query` gives
/// `config.toml`'s `default_query` (TF-617). A query with no recognized `key:value`
/// tokens is treated exactly as before this feature: a case-insensitive substring match
/// against title/identifier, against the raw `query` string byte-for-byte rather than
/// `ParsedQuery::free_text` (which re-joins tokenized text, so isn't guaranteed
/// byte-identical to `query` for e.g. irregular whitespace) — so nothing about TF-580's
/// original behavior changes; see the `matching_issue_indices_*` tests below, which this
/// path keeps passing unmodified (a query wrapped in one pair of double quotes is the one
/// exception: `strip_surrounding_quotes` unwraps it first, so a whole-phrase quoted
/// search behaves the same whether or not the query also happens to contain a `sort:`/
/// filter term that pushes it onto the DSL path below — see that function's doc). A query
/// that *does* contain recognized `priority:`/`state:`/`label:` terms additionally
/// narrows by those terms client-side (`crate::plugin::query::matches_filter_term`).
///
/// **This is *not* independent of `default_query` (TF-617)** — a caveat the earlier
/// version of this doc got backwards. `issues` is whatever was already fetched, and
/// `default_query`'s filter terms are applied server-side *before* that fetch (see
/// `main.rs`'s `load_issues`), so anything `default_query` excluded from the fetch is
/// gone before a `/`-filter ever runs — no query typed here can widen back past it. And
/// when `query` carries no `sort:` of its own, `parsed.sort_keys` is empty, this
/// function's own `matched.sort_by(...)` below is skipped, and the result keeps
/// whatever order `issues` was already in — which is `default_query`'s `sort:` order
/// when one was set (see `main.rs`'s `apply_fetched_issues`), not "fetch order". In
/// short: a `/`-filter narrows and (optionally) reorders *on top of* `default_query`,
/// it does not replace or reset it — README.md's DSL section documents the user-facing
/// version of this.
///
/// Pure and independent of `App`/`ViewState` so it's directly unit-testable, mirroring
/// the `assignee_open_filter`/`project_open_filter` pattern in `data.rs`.
pub fn matching_issue_indices(issues: &[Issue], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..issues.len()).collect();
    }

    let parsed = crate::plugin::query::parse_query(query);
    if parsed.filters.is_empty() && parsed.sort_keys.is_empty() {
        let query = strip_surrounding_quotes(query).to_lowercase();
        return issues
            .iter()
            .enumerate()
            .filter(|(_, issue)| {
                issue.title.to_lowercase().contains(&query)
                    || issue.identifier.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
    }

    let free_text = parsed.free_text.to_lowercase();
    let mut matched: Vec<usize> = issues
        .iter()
        .enumerate()
        .filter(|(_, issue)| {
            (free_text.is_empty()
                || issue.title.to_lowercase().contains(&free_text)
                || issue.identifier.to_lowercase().contains(&free_text))
                && parsed
                    .filters
                    .iter()
                    .all(|term| crate::plugin::query::matches_filter_term(issue, term))
        })
        .map(|(index, _)| index)
        .collect();

    if !parsed.sort_keys.is_empty() {
        matched.sort_by(|&a, &b| {
            crate::plugin::query::compare_issues(&issues[a], &issues[b], &parsed.sort_keys)
        });
    }

    matched
}

/// Strips one pair of surrounding `"` quotes from `query`, if the *entire* string is
/// wrapped in one — TF-617 review fix, so `matching_issue_indices`' legacy substring
/// fast path treats a whole-phrase quoted search the same way `crate::plugin::query`'s
/// tokenizer would (see its doc: a `"..."` span is a single token with the quotes
/// stripped). Before this, `/"fix login"` searched for the six-character literal
/// `"fix login"` — quote characters included — and matched nothing, while
/// `/"fix login" sort:priority` (same phrase, plus an unrelated `sort:` term that
/// pushes the query onto the DSL path and its `ParsedQuery::free_text`, which *is*
/// already quote-stripped) matched correctly — the exact same typed phrase behaving
/// differently depending on what else was in the query. Only single quotes (`'`)
/// aren't handled, matching the tokenizer, which doesn't recognize them as quoting
/// either. Doesn't attempt to strip a quote that doesn't wrap the *whole* query (e.g.
/// `fix "login"`) — the fast path is a single raw substring search with no
/// tokenization, so only the whole-string case has an unambiguous answer.
fn strip_surrounding_quotes(query: &str) -> &str {
    let bytes = query.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        &query[1..query.len() - 1]
    } else {
        query
    }
}

/// What the UI should currently display: the view-selection menu, or an entered view.
// See `ViewState`'s own `#[allow(clippy::large_enum_variant)]` comment — `View`'s size is
// dominated by the same `ViewState::Loaded`/`CommentState` chain.
#[allow(clippy::large_enum_variant)]
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
    /// The currently active tab. `pub(crate)` (follow-up review fix, TF-585): the field
    /// was `pub` on a `pub` struct in a `pub mod`, in a crate that's a real `[lib]`
    /// target (not just this binary's internals) — external code could construct
    /// `HelpOverlayState { tab, scroll }` directly, bypassing every one of `App`'s
    /// invariant-preserving mutators (e.g. the scroll-reset-on-tab-switch below). Every
    /// legitimate reader/mutator of this state lives inside this crate (`app.rs` itself
    /// and `ui.rs`'s render path), so `pub(crate)` loses nothing real.
    pub(crate) tab: HelpTab,
    /// Vertical scroll offset into the active tab's content, in lines. Reset to `0` on
    /// every tab switch — each tab's content is independent, so a scroll position from
    /// one tab is meaningless on another. `pub(crate)` for the same reason as `tab` above.
    pub(crate) scroll: u16,
}

/// A currently active named filter preset (TF-647): its position in the configured
/// `[[filter_presets]]` list (`index` — used to re-resolve the preset's query fresh from
/// `config.toml` on every fetch, see `main.rs`'s `resolved_query_for`, rather than trusting
/// a value cached at cycle time) and its `name` (used only for display — the list title,
/// mirroring how the live `/`-filter query is already shown there). Distinct from
/// `default_query`, which has no name and is never shown; `None` (no `ActivePreset`) means
/// `default_query` (or no query at all) is in effect, exactly as before this feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePreset {
    /// Considered `pub(crate)` (the [`HelpOverlayState`] TF-585 precedent above), but
    /// unlike that type, `ActivePreset` is also constructed and read directly from
    /// `main.rs`'s `resolved_query_for` (and its tests) — and `main.rs` is a separate
    /// crate from this `[lib]` target (it depends on it as `herdr_linear`), so
    /// `pub(crate)` would make those call sites a compile error, not just a warning.
    /// Fields stay `pub` for that structural reason; the actual invariant (`index` must
    /// point at a real preset in a freshly-read `[[filter_presets]]` list) is enforced
    /// where it's checkable — at the point of use in `resolved_query_for`, which
    /// bounds-checks it on every fetch and falls back to `default_query` (clearing the
    /// stale preset) rather than trusting it — not at construction time, since validity
    /// depends on `config.toml`'s state at the moment of use, not at the moment of
    /// construction.
    pub index: usize,
    pub name: String,
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
    /// The currently active named filter preset (TF-647), if any. Lives in parallel to
    /// `screen` (like `status`) rather than nested inside `ViewState::Loaded` — a preset
    /// is a cross-view, sticky setting (it applies to whichever view is entered next,
    /// exactly like `default_query` already does), and nesting it would mean threading it
    /// through `set_issues`'s ~90 existing call sites for no benefit.
    active_preset: Option<ActivePreset>,
}

impl App {
    /// Creates a new application on the menu, first option highlighted.
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
            status: None,
            help_overlay: None,
            active_preset: None,
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
                    comment: CommentState::default(),
                    detail_scroll: 0,
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
                detail_scroll,
                ..
            },
        ) = &mut self.screen
        {
            let matched = matching_issue_indices(issues, &filter.query).len();
            if matched > 0 && *selected + 1 < matched {
                *selected += 1;
                *detail_scroll = 0;
            }
        }
    }

    /// Moves the selection up one position within the filtered subset, if there are
    /// matching issues above. No-op outside a loaded view.
    pub fn move_selection_up(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                selected,
                detail_scroll,
                ..
            },
        ) = &mut self.screen
        {
            if *selected > 0 {
                *selected -= 1;
                *detail_scroll = 0;
            }
        }
    }

    /// Scrolls the Detail pane's content down one row (`j`), clamped so the stored
    /// offset can never advance past the last row of `content_line_count` (the selected
    /// issue's rendered row count — see `ui::detail_line_count`, which callers must pass
    /// since only `ui.rs` knows how the issue's header/Markdown body actually renders).
    /// Mirrors `help_overlay_scroll_down`'s identical clamp-in-`App` rationale: leaving
    /// this unbounded would let a held-down `j` drive the offset far past the content,
    /// leaving the pane blank until an equal number of `k` presses recovered it. No-op
    /// outside a loaded view.
    pub fn detail_scroll_down(&mut self, content_line_count: usize) {
        if let Screen::View(_, ViewState::Loaded { detail_scroll, .. }) = &mut self.screen {
            // `.min(u16::MAX as usize)` before the cast, mirroring `ui::status_banner_
            // height`'s identical guard: an implausibly long description (needing more
            // than 65,535 rendered rows) would otherwise wrap `as u16` around to a small
            // number and *under*-clamp, letting `detail_scroll` advance far past the real
            // content instead of stopping at its true end.
            let max_scroll = content_line_count.saturating_sub(1).min(u16::MAX as usize) as u16;
            *detail_scroll = detail_scroll.saturating_add(1).min(max_scroll);
        }
    }

    /// Scrolls the Detail pane's content up one row (`k`), clamped at the top. No-op
    /// outside a loaded view.
    pub fn detail_scroll_up(&mut self) {
        if let Screen::View(_, ViewState::Loaded { detail_scroll, .. }) = &mut self.screen {
            *detail_scroll = detail_scroll.saturating_sub(1);
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
                ..
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
                filter,
                selected,
                detail_scroll,
                ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.query.push(c);
                *selected = 0;
                *detail_scroll = 0;
            }
        }
    }

    /// Removes the last character of the filter query, if any, and resets selection.
    /// No-op unless currently editing.
    pub fn pop_filter_char(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                filter,
                selected,
                detail_scroll,
                ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.query.pop();
                *selected = 0;
                *detail_scroll = 0;
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
                filter,
                selected,
                detail_scroll,
                ..
            },
        ) = &mut self.screen
        {
            if filter.editing {
                filter.editing = false;
                filter.query.clear();
                *selected = 0;
                *detail_scroll = 0;
            }
        }
    }

    /// True while the current view's comment draft is being edited (`m` pressed, not
    /// yet confirmed or cancelled). False outside a loaded view, and false for a
    /// [`CommentState::Failed`] draft — resumable, but not itself "being edited" until
    /// `m` reopens it.
    fn is_commenting(&self) -> bool {
        matches!(
            &self.screen,
            Screen::View(_, ViewState::Loaded { comment, .. })
                if matches!(comment, CommentState::Editing { .. })
        )
    }

    /// Opens comment editing (bound to `m`) for the currently selected issue. No-op if
    /// there is no selected issue — an empty list, or a filter matching nothing (TF-648's
    /// AC) — or outside a loaded view, since there would be nothing to attach the comment
    /// to. If the last draft for this same issue failed to send
    /// ([`CommentState::Failed`]), resumes it with its text intact instead of starting
    /// blank (PR #53 review) — a transient send failure shouldn't mean retyping the whole
    /// comment. A failed draft for a *different* issue than the one now selected is
    /// simply replaced by a fresh blank draft, not resumed.
    pub fn start_commenting(&mut self) {
        let Some(selected) = self.selected_issue().cloned() else {
            return;
        };
        if let Screen::View(_, ViewState::Loaded { comment, .. }) = &mut self.screen {
            let body = match comment {
                CommentState::Failed { issue, body } if issue.id == selected.id => {
                    std::mem::take(body)
                }
                _ => String::new(),
            };
            *comment = CommentState::Editing {
                issue: selected,
                body,
            };
        }
    }

    /// Appends `c` to the comment draft. No-op unless currently editing.
    pub fn push_comment_char(&mut self, c: char) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                comment: CommentState::Editing { body, .. },
                ..
            },
        ) = &mut self.screen
        {
            body.push(c);
        }
    }

    /// Removes the last character of the comment draft, if any. No-op unless currently
    /// editing.
    pub fn pop_comment_char(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                comment: CommentState::Editing { body, .. },
                ..
            },
        ) = &mut self.screen
        {
            body.pop();
        }
    }

    /// Confirms the comment draft (bound to `Enter` while editing) and, if non-empty once
    /// trimmed, returns an [`Action::AddComment`] for `event_loop` to submit via
    /// `client.add_comment()`. Returns `None` in two distinct cases, neither of which
    /// sends anything: outside `Editing` (nothing to confirm), and *while* `Editing` with
    /// a blank draft (TF-648's AC) — the latter leaves editing open rather than
    /// discarding whatever (nothing) was typed, mirroring how `Esc` is the explicit way
    /// to abandon a draft. Only a non-blank confirm transitions `comment` to
    /// [`CommentState::Idle`] — not cleared-in-place — handing the target `issue` and
    /// trimmed `body` to the returned `Action` rather than re-resolving `selected_issue`
    /// here (see [`CommentState::Editing`]'s doc for why that matters).
    pub fn confirm_comment(&mut self) -> Option<Action> {
        if let Screen::View(_, ViewState::Loaded { comment, .. }) = &mut self.screen {
            if let CommentState::Editing { body, .. } = comment {
                let trimmed = body.trim().to_string();
                if trimmed.is_empty() {
                    return None;
                }
                let CommentState::Editing { issue, .. } =
                    std::mem::replace(comment, CommentState::Idle)
                else {
                    unreachable!("just matched CommentState::Editing above");
                };
                return Some(Action::AddComment {
                    issue,
                    body: trimmed,
                });
            }
        }
        None
    }

    /// Cancels comment editing (bound to `Esc` while editing): stops capturing keystrokes
    /// and discards the draft entirely (not resumable — unlike a send failure, an
    /// explicit `Esc` means the user chose to abandon it). No-op unless currently editing.
    pub fn cancel_comment(&mut self) {
        if let Screen::View(_, ViewState::Loaded { comment, .. }) = &mut self.screen {
            if matches!(comment, CommentState::Editing { .. }) {
                *comment = CommentState::Idle;
            }
        }
    }

    /// Reopens a confirmed comment as a resumable [`CommentState::Failed`] draft after it
    /// failed to send — network/API error, or (see `main.rs`'s `event_loop`) no client
    /// connected yet (PR #53 review: previously the typed text was simply lost on either
    /// path). No-op outside a loaded view; harmless if the user has since navigated
    /// elsewhere within it, since `start_commenting` only resumes a `Failed` draft that
    /// still matches the currently selected issue; a stale one is just never resumed and
    /// is overwritten by the next `set_issues` reload, same as any other draft.
    pub fn restore_failed_comment_draft(&mut self, issue: Issue, body: String) {
        if let Screen::View(_, ViewState::Loaded { comment, .. }) = &mut self.screen {
            *comment = CommentState::Failed { issue, body };
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

    /// The currently active named filter preset (TF-647), if any. `None` means
    /// `default_query` (or no query at all) is in effect — unchanged pre-TF-647 behavior.
    pub fn active_preset(&self) -> Option<&ActivePreset> {
        self.active_preset.as_ref()
    }

    /// Sets the currently active filter preset directly. Used by `main.rs`'s
    /// `resolved_query_for` to fall back to `None` when the preset at the stored `index`
    /// no longer exists (`config.toml`'s `[[filter_presets]]` list shrank since it was
    /// activated) — a targeted correction, not a general-purpose setter for cycling (see
    /// [`Self::cycle_active_preset`] for that).
    pub fn set_active_preset(&mut self, preset: Option<ActivePreset>) {
        self.active_preset = preset;
    }

    /// Cycles to the next configured filter preset (TF-647): `None` (`default_query`) →
    /// preset 0 → preset 1 → … → the last preset → back to `None` → preset 0 again — a
    /// full loop that always includes a "back to baseline" stop, so there's an in-app way
    /// back to `default_query` alone without editing `config.toml` or restarting.
    ///
    /// `presets` is the freshly-read `[[filter_presets]]` list (`main.rs`'s
    /// `event_loop` reads `config.toml` right before calling this, in response to the `p`
    /// key) — an empty list always resets to (and stays at) `None`, which is exactly
    /// TF-647's "no presets configured → unchanged `default_query`-only behavior"
    /// acceptance criterion. Takes the resolved presets themselves (not just a count) so
    /// the new `ActivePreset`'s `name` can be filled in immediately, with no placeholder.
    pub fn cycle_active_preset(&mut self, presets: &[crate::plugin::config::FilterPreset]) {
        if presets.is_empty() {
            self.active_preset = None;
            return;
        }
        let next_index = match &self.active_preset {
            None => Some(0),
            Some(active) if active.index + 1 < presets.len() => Some(active.index + 1),
            Some(_) => None,
        };
        self.active_preset = next_index.map(|index| ActivePreset {
            index,
            name: presets[index].name.clone(),
        });
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
    /// `c` was pressed (any screen or view state — Menu, a view that's still loading, a
    /// loaded view, or the error screen): open `config.toml` (creating the directory and a
    /// starter file first if either is missing) in the user's editor. Handled in
    /// `main.rs`'s `event_loop`, mirroring the existing `OpenInBrowser` → `open::that`
    /// pattern.
    OpenConfig(PathBuf),
    /// `p` was pressed in a view (TF-647): cycle to the next configured named filter
    /// preset and refetch with it. Unlike [`Action::Retry`]/[`Action::EnterView`] — the
    /// other two variants that feed into `main.rs`'s `draw_and_load` — `handle_key`
    /// performs *no* state mutation before returning this one — cycling needs the current
    /// `[[filter_presets]]` list from `config.toml`, and `handle_key` must stay pure/
    /// I/O-free (see its own doc comment). `main.rs`'s `event_loop` does the actual work:
    /// read `config.toml`, call [`App::cycle_active_preset`], then [`App::retry`] +
    /// refetch — a no-op when no presets are configured (TF-647's AC).
    CyclePreset,
    /// Comment composition (bound to `m`, TF-648) reached `Enter` with a non-empty draft
    /// — see [`App::confirm_comment`]: submit `body` on `issue` via `client.add_comment()`.
    /// A named-field variant rather than a positional tuple, unlike this enum's other
    /// multi-field variants, specifically because `issue`/`body` are two same-shaped-enough
    /// values (both ultimately just data attached to a request) that a positional swap at
    /// a call site wouldn't necessarily look wrong at a glance, even though the compiler
    /// would still catch it (the two types differ). Orchestrated in `main.rs`'s
    /// `event_loop`, mirroring `Action::Implement`'s status-banner pattern (an interim
    /// `Status::Ok` while the request is in flight, then `Status::Ok`/`Status::Error` once
    /// it resolves).
    AddComment {
        /// The issue to attach the comment to — captured once when editing started (see
        /// [`CommentState::Editing`]), not re-resolved from the current selection.
        issue: Issue,
        /// The trimmed, non-empty comment text to submit.
        body: String,
    },
}

/// Map a key press to an [`Action`], applying any state change (menu navigation,
/// entering a view, list navigation, retry, returning to the menu) directly to
/// `app`. Returns `None` when the key had no effect or only changed state in place.
///
/// `modifiers` must be threaded through from the real `KeyEvent` (see `main.rs`'s
/// `event_loop`) rather than assumed empty: raw mode delivers `Ctrl+C` as an ordinary
/// `KeyCode::Char('c')` key event, not `SIGINT`, so without checking modifiers it would be
/// indistinguishable from a plain `c` press — which now triggers `Action::OpenConfig`
/// (filesystem writes + spawning an external opener) from any screen or view state: Menu,
/// a view that's still loading, a loaded view, or the error screen. The interrupt reflex is
/// handled unconditionally, before any screen-specific dispatch, so it always quits rather
/// than ever being reinterpreted as a screen's own binding.
///
/// `/` opens type-to-filter on a loaded view's issue list (TF-580); once editing, this
/// function's own filtering branch takes over key dispatch entirely (see
/// `App::is_filtering`) until `Enter` confirms or `Esc` cancels, so none of the
/// ordinary view bindings below (`q`, `o`, `r`, `Enter`-to-implement, ...) fire on a
/// character typed into the filter.
///
/// `m` opens comment composition for the selected issue (TF-648), the same shape as `/`
/// but via `App::is_commenting`/`App::confirm_comment`/`App::cancel_comment` — see their
/// doc comments and `CommentState` for why `Up`/`Down` are excluded here unlike filtering.
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

    if key == KeyCode::Char('?') && !app.is_filtering() && !app.is_commenting() {
        app.open_help_overlay();
        return None;
    }

    // `c` opens `config.toml` — handled once here, before the menu/view dispatch split
    // below, so it works from any screen or view state (Menu, loading, loaded, or error)
    // without needing a `c` arm re-added to every early-return branch. That per-branch
    // duplication is exactly what caused TF-614: `c` only opened config from the error
    // screen because the `in_menu` branch above (and, before this fix, the loaded-view
    // dispatch) had no `c` arm of its own. `modifiers.is_empty()` matters beyond the
    // `Ctrl+C` case already handled above: it also keeps e.g. `Alt+C`/`Shift+C` from
    // triggering config-file writes, since this action's side effect (create + open
    // `config.toml`) is meant for a deliberate, bare `c` press, not any key event that
    // happens to carry the `'c'` character. `!app.is_filtering() && !app.is_commenting()`
    // defers to the filtering/commenting branches below, where a bare `c` must be
    // captured as query/draft text instead (and both are always `false` on the menu
    // screen, so it's safe to check here before the `in_menu` split).
    if key == KeyCode::Char('c')
        && modifiers.is_empty()
        && !app.is_filtering()
        && !app.is_commenting()
    {
        return open_config_action(std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").as_deref());
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

    // Same precedence as the filtering branch above, but for a comment draft (TF-648):
    // character keys are captured into the draft instead of dispatching their usual view
    // bindings. Unlike filtering, `Up`/`Down` are deliberately *not* forwarded to
    // `move_selection_*` here — the draft targets whichever issue was selected when `m`
    // opened editing (see `App::confirm_comment`), and letting navigation continue
    // underneath would risk it silently landing on a different issue than the one shown.
    if app.is_commenting() {
        return match key {
            KeyCode::Enter => app.confirm_comment(),
            KeyCode::Esc => {
                app.cancel_comment();
                None
            }
            KeyCode::Backspace => {
                app.pop_comment_char();
                None
            }
            KeyCode::Char(c) => {
                app.push_comment_char(c);
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
        KeyCode::Char('m') => {
            app.start_commenting();
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
        KeyCode::Char('j') => {
            // `detail_scroll_down` needs `&mut self` to clamp against the selected
            // issue's rendered row count, so that count is computed into a local first
            // (immutable borrow of `app` via `selected_issue`) rather than held across
            // the mutating call — same pattern `handle_help_overlay_key` uses for
            // `help_overlay_scroll_down`'s `HelpTab` lookup.
            let content_line_count = app
                .selected_issue()
                .map(crate::plugin::ui::detail_line_count);
            if let Some(content_line_count) = content_line_count {
                app.detail_scroll_down(content_line_count);
            }
            None
        }
        KeyCode::Char('k') => {
            app.detail_scroll_up();
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
        KeyCode::Char('p') => Some(Action::CyclePreset),
        _ => None,
    }
}

/// Rows the Detail pane scrolls per wheel notch (`handle_mouse`) — a few lines per
/// notch, matching the common terminal/OS wheel convention (herdr's own default for its
/// native pane scrollback, `mouse_scroll_lines`, is also 3) rather than `j`/`k`'s
/// single-row step, which would feel sluggish under a wheel.
const DETAIL_WHEEL_STEP: usize = 3;

/// Dispatches a mouse event. Mouse input is additive to the keyboard-first design —
/// mirroring `herdr-file-viewer`'s own "keyboard-first, mouse additive" rationale for
/// requesting capture at all (see `main.rs::run_tui`) — so only the wheel is handled
/// here; every other kind (clicks, drags) is a deliberate no-op, since `App` has no
/// click-target or divider-drag state to act on one with, and none was requested.
///
/// While the help overlay is open it owns the wheel too (mirrors `handle_help_overlay_key`'s
/// identical rule for the keyboard — otherwise the wheel would silently scroll the `Loaded`
/// view underneath, invisible behind the overlay). Otherwise the wheel scrolls whichever pane
/// the pointer is over: the List (left half of `terminal_width`, matching `ui::draw_view`'s
/// 50/50 horizontal split) moves `selected` one issue per notch — the same step `↑`/`↓` use,
/// since the list has no independent scroll-viewport of its own (`ratatui::List` tracks that
/// internally from `selected`) — and the Detail pane (right half) scrolls
/// `DETAIL_WHEEL_STEP` rows per notch via the same clamped `detail_scroll_down`/
/// `detail_scroll_up` the `j`/`k` keys use. A no-op outside a loaded view (menu, loading,
/// error) — `move_selection_down`/`up`/`detail_scroll_down`/`up` are already no-ops there.
pub fn handle_mouse(
    app: &mut App,
    kind: crossterm::event::MouseEventKind,
    column: u16,
    terminal_width: u16,
) {
    use crossterm::event::MouseEventKind;

    // `HelpTab` is `Copy`, so it's read into a local first rather than holding an
    // immutable borrow of `app` across the mutating scroll call below — same pattern
    // `handle_help_overlay_key` uses for its own `content_line_count` lookup.
    let overlay_tab = app.help_overlay().map(|state| state.tab);
    if let Some(tab) = overlay_tab {
        match kind {
            MouseEventKind::ScrollDown => {
                app.help_overlay_scroll_down(crate::plugin::ui::content_line_count(tab))
            }
            MouseEventKind::ScrollUp => app.help_overlay_scroll_up(),
            _ => {}
        }
        return;
    }

    // While composing a comment, the wheel is a no-op — same "owns the wheel exactly like
    // it already owns the keyboard" rule as the help-overlay branch above, and for the
    // same reason `handle_key`'s commenting branch doesn't forward `Up`/`Down`: moving
    // `selected` out from under an open draft would visually disagree with which issue
    // `App::confirm_comment` will actually post to (PR #53 review — `CommentState::
    // Editing` now captures its target issue once, so a stray scroll can no longer
    // *mis*target the draft, but letting the highlighted row silently drift away from it
    // while the banner still reads "Comment on <the original issue>" would still be a
    // confusing, needless surprise).
    if app.is_commenting() {
        return;
    }

    // `div_ceil`, not plain `/` (floor division): `ui::draw_view`'s two `Constraint::
    // Percentage(50)` chunks split an odd `terminal_width` unevenly — ratatui gives the
    // *first* (List) chunk `ceil(width / 2)` columns and the second (Detail) `floor(width
    // / 2)`. A floor-division boundary here would misclassify the List pane's own last
    // column (index `width / 2`) as Detail on every odd width — verified against the
    // pinned `ratatui` version's actual split behavior (PR #44 review).
    let over_list = column < terminal_width.div_ceil(2);
    match kind {
        MouseEventKind::ScrollDown if over_list => app.move_selection_down(),
        MouseEventKind::ScrollUp if over_list => app.move_selection_up(),
        MouseEventKind::ScrollDown => {
            // Same borrow-timing reason as the overlay branch above: resolve the
            // selected issue's row count into an owned `usize` first.
            let content_line_count = app
                .selected_issue()
                .map(crate::plugin::ui::detail_line_count);
            if let Some(content_line_count) = content_line_count {
                for _ in 0..DETAIL_WHEEL_STEP {
                    app.detail_scroll_down(content_line_count);
                }
            }
        }
        MouseEventKind::ScrollUp => {
            for _ in 0..DETAIL_WHEEL_STEP {
                app.detail_scroll_up();
            }
        }
        _ => {}
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

/// Pure half of the `c` key handling, split out from [`handle_key`] purely
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

    /// The current view's Detail-pane scroll offset, or `None` outside a loaded view.
    fn detail_scroll_of(app: &App) -> Option<u16> {
        match &app.screen {
            Screen::View(_, ViewState::Loaded { detail_scroll, .. }) => Some(*detail_scroll),
            _ => None,
        }
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
    fn detail_scroll_down_increases_offset_and_clamps_at_content_end() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        app.detail_scroll_down(3); // 3 rows of content: offsets 0..=2 are valid
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.detail_scroll_down(3);
        assert_eq!(detail_scroll_of(&app), Some(2));

        app.detail_scroll_down(3); // already at the last row — must not overshoot
        assert_eq!(detail_scroll_of(&app), Some(2));
    }

    #[test]
    fn detail_scroll_up_decreases_offset_and_clamps_at_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.detail_scroll_down(3);
        app.detail_scroll_down(3);
        assert_eq!(detail_scroll_of(&app), Some(2));

        app.detail_scroll_up();
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.detail_scroll_up();
        assert_eq!(detail_scroll_of(&app), Some(0));

        app.detail_scroll_up(); // already at 0 — must not underflow/panic
        assert_eq!(detail_scroll_of(&app), Some(0));
    }

    #[test]
    fn detail_scroll_is_a_no_op_outside_a_loaded_view() {
        let mut app = App::new(); // still on the menu

        app.detail_scroll_down(10);
        app.detail_scroll_up();

        assert_eq!(detail_scroll_of(&app), None);
    }

    #[test]
    fn moving_selection_resets_detail_scroll_to_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.detail_scroll_down(10);
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.move_selection_down();
        assert_eq!(
            detail_scroll_of(&app),
            Some(0),
            "a newly selected issue's description has nothing to do with the old scroll position"
        );

        app.detail_scroll_down(10);
        app.move_selection_up();
        assert_eq!(detail_scroll_of(&app), Some(0));
    }

    #[test]
    fn a_clamped_no_op_selection_move_does_not_disturb_detail_scroll() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]); // single issue: selection can't move
        app.detail_scroll_down(10);
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.move_selection_down(); // no-op: already at the only issue
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.move_selection_up(); // no-op: already at the first issue
        assert_eq!(detail_scroll_of(&app), Some(1));
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
    fn j_key_scrolls_detail_down_and_clamps_at_the_end() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(action, None);
        assert_eq!(detail_scroll_of(&app), Some(1));

        // Enough presses to run well past any real content length — the clamp (via
        // `ui::detail_line_count`, computed from the real selected issue) must hold
        // instead of running away or panicking, mirroring the help overlay's identical
        // TF-585 clamp-in-`App` guard.
        for _ in 0..50 {
            handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        }
        let clamped = detail_scroll_of(&app).unwrap();

        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(
            detail_scroll_of(&app),
            Some(clamped),
            "scroll offset must stay clamped at the content's last row"
        );
    }

    #[test]
    fn k_key_scrolls_detail_up_and_clamps_at_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(detail_scroll_of(&app), Some(2));

        let action = handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(action, None);
        assert_eq!(detail_scroll_of(&app), Some(1));

        handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE); // already at 0
        assert_eq!(detail_scroll_of(&app), Some(0));
    }

    use crossterm::event::{MouseButton, MouseEventKind};

    /// A wheel notch over the left half of an 80-column terminal — the List pane, per
    /// `handle_mouse`'s 50/50 split (matching `ui::draw_view`'s `chunks`).
    const LIST_COLUMN: u16 = 5;
    /// A wheel notch over the right half of the same 80-column terminal — the Detail pane.
    const DETAIL_COLUMN: u16 = 70;
    const TERMINAL_WIDTH: u16 = 80;

    #[test]
    fn mouse_scroll_down_over_the_list_moves_selection() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn mouse_scroll_up_over_the_list_moves_selection_back() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollUp,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn mouse_scroll_over_the_lists_own_last_column_moves_selection_on_an_odd_width() {
        // Regression test (PR #44 review): `ratatui`'s two `Constraint::Percentage(50)`
        // chunks split an odd width unevenly — the List pane (first chunk) gets
        // `ceil(width / 2)` columns, the Detail pane (second chunk) gets `floor(width /
        // 2)`. At width 79 that's List columns 0..=39 (40 columns) and Detail 40..=78. A
        // floor-division boundary (`column < width / 2` = `column < 39`) would
        // misclassify column 39 — the List pane's own last column — as Detail. This must
        // route to the List instead.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        const ODD_TERMINAL_WIDTH: u16 = 79;
        const LISTS_LAST_COLUMN: u16 = 39;

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            LISTS_LAST_COLUMN,
            ODD_TERMINAL_WIDTH,
        );

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        assert_eq!(
            detail_scroll_of(&app),
            Some(0),
            "must not have been misrouted to the Detail pane"
        );
    }

    #[test]
    fn mouse_scroll_down_over_the_detail_pane_scrolls_detail_by_the_wheel_step() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );

        assert_eq!(detail_scroll_of(&app), Some(DETAIL_WHEEL_STEP as u16));
        // Selection itself must be untouched — the wheel over the Detail pane scrolls
        // text, it never re-targets which issue is selected.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn mouse_scroll_up_over_the_detail_pane_scrolls_detail_back_and_clamps_at_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert_eq!(detail_scroll_of(&app), Some(DETAIL_WHEEL_STEP as u16));

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollUp,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert_eq!(detail_scroll_of(&app), Some(0));

        // Already at 0 — must not underflow/panic.
        handle_mouse(
            &mut app,
            MouseEventKind::ScrollUp,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert_eq!(detail_scroll_of(&app), Some(0));
    }

    #[test]
    fn mouse_wheel_is_a_no_op_outside_a_loaded_view() {
        let mut app = App::new(); // still on the menu

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );
        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );

        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn mouse_wheel_is_a_no_op_while_loading_or_on_the_error_screen() {
        // The test above only exercises `Screen::Menu` — this covers the other two
        // "outside a loaded view" screens, reached via a different `Screen::View` arm.
        let mut loading = app_in_my_issues_view(); // Screen::View(_, ViewState::Loading)
        handle_mouse(
            &mut loading,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );
        handle_mouse(
            &mut loading,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert!(matches!(
            loading.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));

        let mut error = app_in_my_issues_view();
        error.set_error("boom".to_string());
        handle_mouse(
            &mut error,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );
        handle_mouse(
            &mut error,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert!(matches!(
            error.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Error { .. })
        ));
    }

    #[test]
    fn mouse_click_is_a_deliberate_no_op() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        // No click/drag support yet (not requested) — every one of these falls through
        // `handle_mouse`'s final `_ => {}` arm, so selection and scroll stay untouched
        // regardless of which specific kind arrives.
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
        ] {
            handle_mouse(&mut app, kind, LIST_COLUMN, TERMINAL_WIDTH);
        }

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
        assert_eq!(detail_scroll_of(&app), Some(0));
    }

    #[test]
    fn mouse_wheel_scrolls_the_help_overlay_when_open_regardless_of_column() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.open_help_overlay();

        // Over what would be the List half were the overlay closed — it must still hit
        // the overlay, not fall through to the Loaded view underneath.
        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );
        assert_eq!(app.help_overlay().unwrap().scroll, 1);

        // Over what would be the Detail half — must hit the same overlay, not the Detail
        // pane's own scroll. If the column check ever leaked into this branch, this would
        // land on `detail_scroll` instead of continuing to advance `help_overlay().scroll`.
        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            DETAIL_COLUMN,
            TERMINAL_WIDTH,
        );
        assert_eq!(app.help_overlay().unwrap().scroll, 2);

        // The hidden Loaded view underneath must be untouched throughout.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
        assert_eq!(detail_scroll_of(&app), Some(0));
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
    fn set_issues_resets_detail_scroll_left_over_from_a_previous_list() {
        // Mirrors `set_issues_clears_marks_left_over_from_a_previous_list`: a second
        // `set_issues` call (retry/re-fetch) must reset `detail_scroll` alongside
        // `issues`/`selected`/`marked`, not just on first entry into the view — otherwise a
        // stale scroll offset would open the newly-fetched first issue's description
        // mid-scroll instead of at the top.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.detail_scroll_down(10);
        assert_eq!(detail_scroll_of(&app), Some(1));

        app.set_issues(vec![sample_issue("ENG-3")]);

        assert_eq!(detail_scroll_of(&app), Some(0));
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

    fn sample_preset(name: &str, query: &str) -> crate::plugin::config::FilterPreset {
        crate::plugin::config::FilterPreset {
            name: name.to_string(),
            query: query.to_string(),
        }
    }

    #[test]
    fn cycle_active_preset_is_a_no_op_when_no_presets_are_configured() {
        let mut app = App::new();

        app.cycle_active_preset(&[]);

        assert_eq!(app.active_preset(), None);
    }

    #[test]
    fn cycle_active_preset_resets_to_none_when_the_preset_list_becomes_empty() {
        let mut app = App::new();
        app.cycle_active_preset(&[sample_preset("Urgent", "priority:>=2")]);
        assert!(app.active_preset().is_some());

        app.cycle_active_preset(&[]);

        assert_eq!(app.active_preset(), None);
    }

    #[test]
    fn cycle_active_preset_loops_through_every_preset_then_back_to_none() {
        let mut app = App::new();
        let presets = [
            sample_preset("Urgent", "priority:>=2"),
            sample_preset("In Review", "state:\"In Review\""),
        ];
        assert_eq!(app.active_preset(), None);

        app.cycle_active_preset(&presets);
        assert_eq!(
            app.active_preset(),
            Some(&ActivePreset {
                index: 0,
                name: "Urgent".to_string(),
            })
        );

        app.cycle_active_preset(&presets);
        assert_eq!(
            app.active_preset(),
            Some(&ActivePreset {
                index: 1,
                name: "In Review".to_string(),
            })
        );

        app.cycle_active_preset(&presets);
        assert_eq!(app.active_preset(), None);

        app.cycle_active_preset(&presets);
        assert_eq!(
            app.active_preset(),
            Some(&ActivePreset {
                index: 0,
                name: "Urgent".to_string(),
            })
        );
    }

    #[test]
    fn set_active_preset_overrides_directly() {
        let mut app = App::new();
        app.cycle_active_preset(&[sample_preset("Urgent", "priority:>=2")]);

        app.set_active_preset(None);

        assert_eq!(app.active_preset(), None);
    }

    #[test]
    fn p_key_in_a_loaded_view_returns_cycle_preset_without_mutating_app_state() {
        let mut app = app_in_my_issues_view();
        let before = app.active_preset().cloned();

        let action = handle_key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);

        assert_eq!(action, Some(Action::CyclePreset));
        assert_eq!(app.active_preset().cloned(), before);
    }

    #[test]
    fn p_key_on_the_menu_does_nothing() {
        let mut app = App::new();

        let action = handle_key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);

        assert_eq!(action, None);
    }

    #[test]
    fn p_key_while_filtering_is_captured_as_filter_text_not_a_preset_cycle() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.start_filtering();

        let action = handle_key(&mut app, KeyCode::Char('p'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert_eq!(app.active_preset(), None);
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
    // is a pure function taking an injected `Option<&OsStr>`, tested directly here. The
    // `handle_key` tests that need to observe the *real* `c`-opens-config wiring end to end
    // (one per screen/view state: menu, a view still loading, a loaded view, error screen)
    // mutate the process environment deliberately and guard it with `ENV_LOCK` — see that
    // lock's comment. The `modified_c_on_*` negative tests below reuse the same env-var
    // setup for the same reason spelled out on `c_key_in_error_state_opens_config_when_dir_is_set`:
    // without a known-non-None expectation to fail against, `None` would pass vacuously even
    // if the `modifiers.is_empty()` guard were deleted.

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
    /// each other, since Rust runs tests in the same binary concurrently by default and an
    /// unguarded env var would be a data race across those tests.
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
        // same env var `handle_key` just consulted — so a regression that re-adds a
        // screen guard on this arm (or deletes the arm entirely, falling through to
        // `_ => None`) actually fails this test instead of passing vacuously.
        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
        );
    }

    #[test]
    fn c_key_on_menu_opens_config_when_dir_is_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = App::new();
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
        );
    }

    #[test]
    fn c_key_on_loaded_view_opens_config_when_dir_is_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
        );
    }

    #[test]
    fn c_key_while_loading_opens_config_when_dir_is_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        // `app_in_my_issues_view()` deliberately skips `set_issues`/`set_error`, leaving the
        // view in `ViewState::Loading` — the fourth state the hoisted `c` check above the
        // menu/view dispatch split covers, alongside menu, loaded, and error.
        let mut app = app_in_my_issues_view();
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(
            action,
            Some(Action::OpenConfig(PathBuf::from(
                "/fake/config/dir/config.toml"
            )))
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
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        // Alt+C (or any other modifier combination besides the Ctrl+C handled above) must
        // not be treated as the bare `c` keybinding — it's not the deliberate press this
        // side-effecting action is meant for. `HERDR_PLUGIN_CONFIG_DIR` is set deliberately
        // (mirroring `c_while_filtering_is_captured_as_query_text_not_open_config`'s
        // comment): with it unset, `open_config_action` would return `None` regardless of
        // the `modifiers.is_empty()` guard, making this assertion pass vacuously even if
        // the guard were deleted.
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::ALT);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(action, None);
    }

    #[test]
    fn modified_c_on_menu_does_not_open_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = App::new();

        // Same guard, exercised on the menu screen, with the same non-vacuous setup as
        // `modified_c_on_error_screen_does_not_open_config`: `c` opening config is now
        // global, but it must still be a bare, unmodified press everywhere it fires.
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::ALT);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(action, None);
    }

    #[test]
    fn modified_c_on_loaded_view_does_not_open_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        // Same guard, exercised on a loaded (non-error) view, with the same non-vacuous
        // setup as the sibling tests above.
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::ALT);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(action, None);
    }

    #[test]
    fn c_while_help_overlay_is_open_does_not_open_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        // The overlay early-return in `handle_key` must keep owning `c` while it's open,
        // same non-vacuous env-var reasoning as the `modified_c_on_*` tests above: the
        // overlay guard runs before the hoisted `c` check, so this also pins down that
        // ordering.
        let mut app = App::new();
        app.open_help_overlay();
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(action, None);
        assert!(app.help_overlay().is_some());
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

    // TF-617: `/`-filter DSL awareness. `matching_issue_indices` recognizes the same
    // `priority:`/`state:`/`label:`/`sort:` tokens `default_query` does, narrowing and
    // reordering the already-loaded `issues` client-side, while a query with none of
    // those tokens still takes the exact legacy path exercised above.

    #[test]
    fn matching_issue_indices_filters_by_a_recognized_priority_term() {
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].priority = 2;
        issues[1].priority = 0;

        assert_eq!(matching_issue_indices(&issues, "priority:>=2"), vec![0]);
    }

    #[test]
    fn matching_issue_indices_filters_by_a_recognized_state_term() {
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].state.name = "Done".to_string();
        issues[1].state.name = "In Progress".to_string();

        assert_eq!(matching_issue_indices(&issues, "state:done"), vec![0]);
    }

    #[test]
    fn matching_issue_indices_combines_free_text_with_a_filter_term() {
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        issues[0].priority = 2;
        issues[1].title = "Fix login redirect".to_string();
        issues[1].priority = 0;

        assert_eq!(
            matching_issue_indices(&issues, "login priority:>=2"),
            vec![0]
        );
    }

    #[test]
    fn matching_issue_indices_sorts_matches_by_a_recognized_sort_term() {
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].priority = 1;
        issues[1].priority = 3;

        assert_eq!(
            matching_issue_indices(&issues, "sort:-priority"),
            vec![1, 0]
        );
    }

    #[test]
    fn matching_issue_indices_with_no_dsl_tokens_takes_the_exact_legacy_substring_path() {
        // A trailing space is significant to the legacy substring match (it's part of
        // the exact byte string being searched for) but would be dropped by re-joining
        // `ParsedQuery::free_text`'s tokens — confirms the raw `query` string, not the
        // parsed-and-rejoined one, still drives this path.
        let mut issues = vec![sample_issue("ENG-1")];
        issues[0].title = "Fix login bug ".to_string();

        assert_eq!(matching_issue_indices(&issues, "bug "), vec![0]);
    }

    #[test]
    fn matching_issue_indices_strips_a_whole_query_wrapped_in_double_quotes() {
        // TF-617 review fix: previously the fast path searched for the literal
        // `"fix login"` (quote characters included) and matched nothing, while
        // `"fix login" sort:priority` (same phrase, plus an unrelated `sort:` that
        // pushes the query onto the DSL path and its already-quote-stripped
        // `ParsedQuery::free_text`) matched correctly — same typed phrase, inconsistent
        // result depending on what else was in the query.
        let mut issues = vec![sample_issue("ENG-1")];
        issues[0].title = "Fix login bug".to_string();

        assert_eq!(matching_issue_indices(&issues, "\"fix login\""), vec![0]);
    }

    #[test]
    fn matching_issue_indices_does_not_strip_a_quote_that_does_not_wrap_the_whole_query() {
        // Only the whole-string-wrapped case is handled (see
        // `strip_surrounding_quotes`'s doc) — a query that isn't wrapped start-to-end in
        // quotes is searched for literally, quote characters included, same as before
        // this fix. The query here has trailing text after its closing quote, so the
        // literal `"` characters stay in the search string and this does not match a
        // title that (realistically) doesn't itself contain a `"`.
        let mut issues = vec![sample_issue("ENG-1")];
        issues[0].title = "hello extra text".to_string();

        assert_eq!(
            matching_issue_indices(&issues, "\"hello\" extra"),
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
    fn c_while_filtering_is_captured_as_query_text_not_open_config() {
        // TF-614 regression: `c` now opens config.toml from any screen, but a literal `c`
        // typed into the filter query must stay filter text. `HERDR_PLUGIN_CONFIG_DIR` is
        // set deliberately, mirroring the other `c`-key tests' comment: an unset env var
        // would make `None` a vacuous pass even if the filtering guard were deleted and
        // `c` fell through to the general match's (env-var-gated) `OpenConfig` arm.
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

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
    fn typing_a_filter_character_resets_detail_scroll_to_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.detail_scroll_down(10);
        assert_eq!(detail_scroll_of(&app), Some(1));

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);

        assert_eq!(detail_scroll_of(&app), Some(0));
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
    fn backspace_while_filtering_resets_detail_scroll_to_zero() {
        // Mirrors `typing_a_filter_character_resets_detail_scroll_to_zero` — `pop_filter_char`
        // resets `selected` exactly like `push_filter_char` does, so it must reset
        // `detail_scroll` alongside it too, for the same reason: the narrowed-then-widened
        // match set can land `selected` on a different issue than before.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        app.detail_scroll_down(10);
        assert_eq!(detail_scroll_of(&app), Some(1));

        handle_key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);

        assert_eq!(detail_scroll_of(&app), Some(0));
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
    fn esc_while_filtering_resets_detail_scroll_to_zero() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.detail_scroll_down(10);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(detail_scroll_of(&app), Some(1));

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(detail_scroll_of(&app), Some(0));
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
    fn typing_a_recognized_dsl_term_through_handle_key_narrows_the_view_end_to_end() {
        // TF-617: end-to-end confirmation that `/`-filter editing (start_filtering via
        // `/`, push_filter_char per character) feeds straight into the DSL-aware
        // matching_issue_indices — not just a direct unit-test of that function.
        let mut app = app_in_my_issues_view();
        let mut low = sample_issue("ENG-1");
        low.priority = 1;
        let mut high = sample_issue("ENG-2");
        high.priority = 2;
        app.set_issues(vec![low, high]);

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "priority:>=2".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    // TF-617 review fixes: composition-level regression tests for how a `/`-filter and
    // `default_query` (applied server-side/pre-sorted by the time `issues` lands in
    // `App` — see `main.rs`'s `load_issues`/`apply_fetched_issues`) interact. These
    // pin the corrected `matching_issue_indices` doc/README claim: a `/`-filter
    // narrows and (optionally) re-sorts *on top of* `default_query`, it never replaces
    // or resets it.

    #[test]
    fn slash_filter_cannot_widen_past_what_default_query_already_excluded_from_the_fetch() {
        // `default_query`'s filter terms narrow the fetch itself (main.rs's
        // `load_issues`), so an excluded issue is never in `issues` to begin with — no
        // `/`-filter query, however permissive, can bring it back. Simulates
        // `default_query = "priority:>=2"` having already excluded a low-priority issue
        // from the fetch, by simply never including one in `issues` here.
        let mut app = app_in_my_issues_view();
        let mut high = sample_issue("ENG-2");
        high.priority = 3;
        app.set_issues(vec![high]); // the hypothetical low-priority issue was never fetched

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "priority:<=1".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue(), None);
    }

    #[test]
    fn slash_filter_without_a_sort_term_of_its_own_preserves_the_already_loaded_order() {
        // A `/`-filter with no `sort:` doesn't revert to "fetch order" —
        // `matching_issue_indices` only reorders when the *typed* query has its own
        // `sort:` (its `matched.sort_by(...)` step is skipped otherwise), so whatever
        // order `issues` was already in — e.g. `default_query`'s own `sort:`, applied at
        // fetch time via `apply_fetched_issues` — survives narrowing untouched.
        let mut app = app_in_my_issues_view();
        let mut high = sample_issue("ENG-2");
        high.priority = 3;
        let mut medium = sample_issue("ENG-3");
        medium.priority = 2;
        // Pre-sorted priority-descending, as `apply_fetched_issues` would leave it for
        // `default_query = "sort:-priority"` — deliberately not insertion order.
        app.set_issues(vec![high, medium]);

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "eng".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-3");
    }

    #[test]
    fn slash_filter_requires_all_of_multiple_recognized_filter_terms_at_once() {
        // `matching_issue_indices`' DSL path ANDs every recognized filter term together
        // (`parsed.filters.iter().all(...)`) — every other test exercising this path
        // only ever used a single term, so a regression to OR-combining (any term
        // matches) would have passed them all. Three issues, each satisfying at most
        // one of the two typed terms except the last, which satisfies both.
        let mut app = app_in_my_issues_view();
        let mut state_only = sample_issue("ENG-1");
        state_only.state.name = "Done".to_string();
        state_only.priority = 0;
        let mut priority_only = sample_issue("ENG-2");
        priority_only.priority = 2;
        let mut both = sample_issue("ENG-3");
        both.state.name = "Done".to_string();
        both.priority = 2;
        app.set_issues(vec![state_only, priority_only, both]);

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "state:done priority:>=2".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-3");
        // Only the one issue satisfies both terms — moving "down" within the matched
        // set is a no-op, confirming neither of the other two slipped through.
        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-3");
    }

    #[test]
    fn slash_filter_narrows_and_sorts_together_in_one_query() {
        let mut app = app_in_my_issues_view();
        let mut low = sample_issue("ENG-1");
        low.priority = 2;
        let mut high = sample_issue("ENG-2");
        high.priority = 4;
        let mut excluded = sample_issue("ENG-3");
        excluded.priority = 0;
        app.set_issues(vec![low, high, excluded]); // insertion order: low, high, excluded

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "priority:>=1 sort:-priority".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        // `excluded` (priority 0) fails the filter half; the two survivors are
        // reordered priority-descending by the query's own `sort:`, not left in
        // insertion/fetch order.
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn esc_restores_default_query_shaped_order_not_fetch_order() {
        // README's "Esc ... restores default_query's view" claim — regression test
        // using a list that's already sorted the way `apply_fetched_issues` would leave
        // it for `default_query = "sort:-priority"`, confirming `cancel_filter` doesn't
        // touch `issues` at all, so whatever order it was already in survives.
        let mut app = app_in_my_issues_view();
        let mut high = sample_issue("ENG-2");
        high.priority = 3;
        let mut low = sample_issue("ENG-1");
        low.priority = 1;
        app.set_issues(vec![high, low]); // pre-sorted priority-descending

        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "eng".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
        app.move_selection_down();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
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

    // --- TF-648: comment composition (`m`) -------------------------------------------

    #[test]
    fn m_key_starts_commenting() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_commenting());
    }

    #[test]
    fn m_key_outside_a_loaded_view_does_nothing() {
        let mut app = app_in_my_issues_view(); // still Loading, not Loaded

        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        assert!(!app.is_commenting());
    }

    #[test]
    fn m_key_with_no_selected_issue_does_nothing() {
        // AC (TF-648): "kein Issue selektiert/leere Liste → No-op" — an empty list has no
        // issue for the draft to target, so `m` must not open editing.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        assert!(!app.is_commenting());
    }

    #[test]
    fn m_key_when_the_active_filter_matches_nothing_does_nothing() {
        // Same AC, reached via a narrowed-to-empty filter rather than a literally empty
        // list — `selected_issue()` is `None` either way.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "nonexistent".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // confirm, stop editing

        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        assert!(!app.is_commenting());
    }

    #[test]
    fn typing_while_commenting_appends_to_the_draft_instead_of_triggering_its_usual_binding() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        // 'q' would normally quit — while commenting it must be captured as text instead.
        let action = handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_commenting());
    }

    #[test]
    fn c_while_commenting_is_captured_as_draft_text_not_open_config() {
        // Mirrors `c_while_filtering_is_captured_as_query_text_not_open_config`: `c`
        // opens config.toml from any screen, but typed into an open comment draft it
        // must stay draft text. `HERDR_PLUGIN_CONFIG_DIR` is set deliberately so `None`
        // isn't a vacuous pass if the commenting guard were ever deleted.
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR");
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", "/fake/config/dir");

        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        let action = handle_key(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);

        match previous {
            Some(value) => std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", value),
            None => std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR"),
        }

        assert_eq!(action, None);
        assert!(app.is_commenting());
    }

    #[test]
    fn question_mark_while_commenting_types_into_the_draft_instead_of_opening_the_overlay() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        assert!(app.help_overlay().is_none());
        assert!(app.is_commenting());
    }

    #[test]
    fn up_and_down_while_commenting_do_not_move_the_selection() {
        // Unlike filtering, navigation must not leak through while composing a comment:
        // the draft targets whichever issue was selected when `m` opened editing, and
        // silently moving `selected` underneath it could post the comment to the wrong
        // issue. See `CommentState`'s doc comment.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE);

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
        assert!(app.is_commenting());
    }

    #[test]
    fn backspace_while_commenting_removes_the_last_character() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('!'), KeyModifiers::NONE); // typo

        handle_key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::AddComment {
                issue: sample_issue("ENG-1"),
                body: "hi".to_string()
            })
        );
    }

    #[test]
    fn enter_with_an_empty_draft_does_not_send_and_stays_in_editing_mode() {
        // AC (TF-648): "Ein leerer Kommentar wird nicht abgesendet" — nothing was typed,
        // so confirming must be a no-op rather than sending a blank comment or silently
        // discarding the (empty) draft the way `Esc` would.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_commenting());
    }

    #[test]
    fn enter_with_only_whitespace_does_not_send() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        for c in "   ".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_commenting());
    }

    #[test]
    fn enter_confirms_a_non_empty_comment_and_returns_an_add_comment_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        for c in "Looks good".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::AddComment {
                issue: sample_issue("ENG-1"),
                body: "Looks good".to_string()
            })
        );
        assert!(!app.is_commenting());
    }

    #[test]
    fn enter_trims_leading_and_trailing_whitespace_from_the_sent_comment() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        for c in "  padded  ".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::AddComment {
                issue: sample_issue("ENG-1"),
                body: "padded".to_string()
            })
        );
    }

    #[test]
    fn esc_while_commenting_cancels_and_discards_the_draft() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        for c in "throwaway".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(!app.is_commenting());

        // Reopening editing must not resurrect the discarded draft.
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(action, None); // draft is empty again — nothing to send
    }

    #[test]
    fn esc_after_confirming_a_comment_falls_through_to_its_usual_binding() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // confirm, stop editing

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        // No longer editing, so Esc falls through to its ordinary "back to menu" binding.
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn reentering_a_view_clears_any_previous_comment_draft() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);

        // A fresh fetch (e.g. `r`etry, or re-entering the view) rebuilds `Loaded` from
        // scratch via `set_issues`, which must not carry a stale draft along with it.
        app.set_issues(vec![sample_issue("ENG-1")]);

        assert!(!app.is_commenting());
    }

    #[test]
    fn mouse_wheel_while_commenting_does_not_move_the_selection() {
        // Regression test (PR #53 review): `handle_mouse` used to have no
        // `is_commenting()` guard, so a wheel scroll during comment composition could
        // move `selected` out from under an open draft — even though `CommentState::
        // Editing` now captures its target issue up front and can no longer be
        // *mis*targeted by it (see that type's doc comment), letting the highlighted row
        // silently drift away from the issue the banner names would still be a confusing,
        // needless surprise. Mirrors `up_and_down_while_commenting_do_not_move_the_
        // selection`, but for the wheel instead of the arrow keys.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        handle_mouse(
            &mut app,
            MouseEventKind::ScrollDown,
            LIST_COLUMN,
            TERMINAL_WIDTH,
        );

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
        assert!(app.is_commenting());
    }

    #[test]
    fn m_while_filtering_is_captured_as_filter_text_not_commenting() {
        // Reverse case of `c_while_commenting_is_captured_as_draft_text_not_open_config`
        // and `question_mark_while_commenting_types_into_the_draft_instead_of_opening_
        // the_overlay` (PR #53 review): those guard `is_filtering()`'s precedence over
        // the commenting branch below it in `handle_key`. This guards the other
        // direction — `is_filtering()` is checked *first*, so a bare `m` typed into an
        // in-progress filter query must stay filter text, not fall through to `start_
        // commenting` and open a second, conflicting input mode on top of the filter.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);

        let action = handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_filtering());
        assert!(!app.is_commenting());
    }

    #[test]
    fn confirming_a_comment_while_filtered_targets_the_filtered_selection() {
        // Integration test (PR #53 review): `App::start_commenting` captures its target
        // via `selected_issue()`, which itself indexes the *filtered* subset
        // (`matching_issue_indices`), not raw `issues` — prove the two features actually
        // compose, not just that each is independently correct, by filtering
        // `[ENG-1, ENG-2]` down to `ENG-2` and confirming the resulting
        // `Action::AddComment` names `ENG-2`.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "ENG-2".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE); // confirm filter
        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        for c in "lgtm".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::AddComment {
                issue: sample_issue("ENG-2"),
                body: "lgtm".to_string()
            })
        );
    }

    #[test]
    fn m_key_resumes_a_failed_draft_for_the_same_issue() {
        // PR #53 review: a comment that fails to send (network/API error, or — per
        // `main.rs`'s `add_comment_unavailable_status` — no client yet) is reopened as
        // `CommentState::Failed` by `App::restore_failed_comment_draft` rather than
        // discarded. Pressing `m` again on the *same* issue should resume it instead of
        // starting blank.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.restore_failed_comment_draft(sample_issue("ENG-1"), "half-typed".to_string());

        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('!'), KeyModifiers::NONE);
        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            action,
            Some(Action::AddComment {
                issue: sample_issue("ENG-1"),
                body: "half-typed!".to_string()
            })
        );
    }

    #[test]
    fn m_key_does_not_resume_a_failed_draft_for_a_different_issue() {
        // A `Failed` draft is scoped to the issue it was for — selecting a different
        // issue and pressing `m` must start a fresh, blank draft, not silently attach the
        // old failed text to the wrong issue.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.restore_failed_comment_draft(sample_issue("ENG-1"), "half-typed".to_string());
        app.move_selection_down(); // ENG-2 now selected

        handle_key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        // A blank draft doesn't send — this only passes if the ENG-1 body above was
        // *not* carried over into the new draft.
        let action = handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(action, None);
        assert!(app.is_commenting());
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

    /// Follow-up review fix (TF-585): every other scroll test either scrolls on the
    /// default `WhatsNew` tab (via `handle_key`, so it already routes through the real
    /// `ui::content_line_count`) or scrolls via `App::help_overlay_scroll_down` directly
    /// with a hand-picked literal (bypassing `ui::content_line_count` entirely). Neither
    /// exercises the real `content_line_count(HelpTab::Keybindings)` /
    /// `content_line_count(HelpTab::Settings)` production wiring the `j`/`k` handler
    /// actually calls for those tabs. This test does, via `handle_key`, for
    /// `Keybindings` — a purely static, env-independent tab, so the assertions are exact
    /// rather than merely "didn't panic".
    #[test]
    fn j_and_k_scroll_while_the_overlay_is_open_on_the_keybindings_tab() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE); // -> Keybindings
        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::Keybindings);

        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().scroll, 2);

        handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().scroll, 1);
    }

    /// Companion to the above for the `Settings` tab specifically — the one tab whose
    /// content depends on the real environment (`settings_lines()` reads
    /// `HERDR_PLUGIN_CONFIG_DIR`/`LINEAR_API_KEY`). Deliberately doesn't mutate those env
    /// vars (this crate's established convention — see `config.rs`'s untested impure
    /// wrappers — avoids the flakiness of env-var mutation across parallel test threads),
    /// so this only asserts scroll moves at all and never goes negative, not exact
    /// offsets; `settings_lines_from` (tested separately, with injected data) always
    /// produces at least a handful of lines regardless of what's actually resolved, so
    /// `content_line_count(HelpTab::Settings)` is always callable here without panicking.
    #[test]
    fn j_and_k_scroll_while_the_overlay_is_open_on_the_settings_tab() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('3'), KeyModifiers::NONE); // -> Settings
        assert_eq!(app.help_overlay().unwrap().tab, HelpTab::Settings);

        handle_key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        let after_down = app.help_overlay().unwrap().scroll;
        assert!(after_down >= 1, "expected `j` to move scroll off zero");

        handle_key(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.help_overlay().unwrap().scroll, after_down - 1);
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
