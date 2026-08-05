# Plugin View Switcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the plugin's hard-wired "My Issues"-only startup with a menu-first view switcher (My Issues / Project Issues / Team Issues), laying the plumbing TF-578/TF-579 plug into.

**Architecture:** Nest today's data-loading state machine (`Loading`/`Loaded`/`Error`, renamed `ViewState`) inside a new outer `Screen` enum (`Menu` vs `View(ViewKind, ViewState)`), so menu navigation and data-loading stay independently testable. `App::new()` now starts on the menu; only `MyIssues` is selectable until TF-578/TF-579 land.

**Tech Stack:** Rust, ratatui/crossterm (behind the `plugin` Cargo feature), tokio.

## Global Constraints

- Rust edition 2021, MSRV 1.70 (`Cargo.toml` `rust-version`).
- All plugin code lives behind the `plugin` Cargo feature — build/test with `--features plugin` or `--all-features`.
- Must pass the project's quality gate: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features -- --nocapture` (`just fmt` / `just lint` / `just test` / `just check`).
- Commit messages follow this repo's existing plain conventional-commit style (`feat: ...`, `docs: ...`, lower-case, no emoji, no scope required) — see `git log`. Do not use the emoji-conventional-commits style from generic templates.
- Design spec: `docs/superpowers/specs/2026-08-05-view-switcher-design.md`. Linear: [TF-576](https://linear.app/talent-factory/issue/TF-576).
- Out of scope, do not implement here: CWD → Linear-project detection (TF-577), actually fetching Project/Team issues (TF-578/TF-579), in-list search (TF-580), issue creation (TF-581). `ProjectIssues`/`TeamIssues` stay visible-but-disabled in the menu.

---

### Task 1: Restructure `app.rs` into `Screen`/`ViewKind`/`ViewState` and update `ui.rs` rendering

**Files:**
- Modify: `src/plugin/app.rs` (full rewrite of the non-test and test code below `use crate::Issue;`)
- Modify: `src/plugin/ui.rs` (full rewrite of `draw()` and its test module)

**Interfaces:**
- Produces (used by Task 2 / `main.rs`):
  - `pub enum ViewKind { MyIssues, ProjectIssues, TeamIssues }` (`Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct App { pub screen: Screen }`
  - `impl App { pub fn new() -> Self; pub fn current_view(&self) -> Option<ViewKind>; pub fn set_issues(&mut self, issues: Vec<Issue>); pub fn set_error(&mut self, message: String); pub fn retry(&mut self); pub fn selected_issue(&self) -> Option<&Issue>; }`
  - `pub enum Action { Quit, OpenInBrowser(String), Retry, EnterView }` (`Debug, Clone, PartialEq`)
  - `pub fn handle_key(app: &mut App, key: crossterm::event::KeyCode) -> Option<Action>`
  - `pub fn draw(frame: &mut ratatui::Frame, app: &App)` (in `ui.rs`, signature unchanged)

- [ ] **Step 1: Replace the `#[cfg(test)] mod tests` block in `src/plugin/app.rs`**

This references `Screen`, `ViewKind`, `ViewState`, `Action::EnterView`, and `App::enter_selected_menu_option`/`move_menu_selection_down`/`move_menu_selection_up` before they exist — that's expected, see Step 2.

Replace everything from `#[cfg(test)]` to the end of the file with:

```rust
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
    fn entering_an_unavailable_option_does_nothing() {
        let mut app = App::new();
        app.move_menu_selection_down(); // -> Project Issues, unavailable

        let action = app.enter_selected_menu_option();

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 1 }));
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

    use crossterm::event::KeyCode;

    #[test]
    fn down_key_moves_selection_and_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Down);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn up_key_moves_selection_and_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        let action = handle_key(&mut app, KeyCode::Up);

        assert_eq!(action, None);
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn o_key_returns_open_in_browser_with_the_selected_issue_url() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Char('o'));

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

        assert_eq!(handle_key(&mut app, KeyCode::Char('o')), None);
    }

    #[test]
    fn q_key_from_the_menu_returns_quit() {
        let mut app = App::new();

        assert_eq!(handle_key(&mut app, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn esc_key_from_the_menu_returns_quit() {
        let mut app = App::new();

        assert_eq!(handle_key(&mut app, KeyCode::Esc), Some(Action::Quit));
    }

    #[test]
    fn enter_key_on_the_default_menu_selection_enters_my_issues() {
        let mut app = App::new();

        let action = handle_key(&mut app, KeyCode::Enter);

        assert_eq!(action, Some(Action::EnterView));
        assert!(matches!(
            app.screen,
            Screen::View(ViewKind::MyIssues, ViewState::Loading)
        ));
    }

    #[test]
    fn enter_key_on_an_unavailable_menu_option_does_nothing() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Down); // -> Project Issues, unavailable

        let action = handle_key(&mut app, KeyCode::Enter);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 1 }));
    }

    #[test]
    fn q_key_from_a_view_returns_quit() {
        let mut app = app_in_my_issues_view();

        assert_eq!(handle_key(&mut app, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn esc_key_from_a_loaded_view_returns_to_the_menu() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let action = handle_key(&mut app, KeyCode::Esc);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn esc_key_from_an_error_view_returns_to_the_menu() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        let action = handle_key(&mut app, KeyCode::Esc);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
    }

    #[test]
    fn r_key_in_error_state_retries_and_returns_retry_action() {
        let mut app = app_in_my_issues_view();
        app.set_error("boom".to_string());

        let action = handle_key(&mut app, KeyCode::Char('r'));

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

        assert_eq!(handle_key(&mut app, KeyCode::Char('r')), None);
    }
}
```

- [ ] **Step 2: Confirm it fails to compile**

Run: `cargo test --lib --features plugin -- --nocapture`
Expected: FAIL — `error[E0433]: failed to resolve: use of undeclared type Screen` (and similar for `ViewKind`, `ViewState`, `Action::EnterView`, missing methods). This is expected: the test module now names types/methods that don't exist yet.

- [ ] **Step 3: Replace the production code in `src/plugin/app.rs`**

Replace everything from the top of the file through (but not including) `#[cfg(test)]` with:

```rust
//! TUI application state and navigation.
//!
//! Provides pure state management for the terminal UI without any rendering logic.
//! The app starts on a menu (`Screen::Menu`) offering the available issue views, then
//! moves into `Screen::View` once one is selected — tracking that view's own display
//! state (loading, loaded issues, error) and navigation within its issue list.

use crate::Issue;

/// The views selectable from the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// Issues assigned to the authenticated user.
    MyIssues,
    /// All open issues in the current project (not yet implemented — see TF-577/TF-578).
    ProjectIssues,
    /// All open issues in a team (not yet implemented — see TF-579).
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

/// The menu options in display order, paired with whether they're selectable yet.
/// `ProjectIssues`/`TeamIssues` become available once TF-578/TF-579 implement their
/// data fetching — until then the menu shows but disables them.
pub const MENU_OPTIONS: [(ViewKind, bool); 3] = [
    (ViewKind::MyIssues, true),
    (ViewKind::ProjectIssues, false),
    (ViewKind::TeamIssues, false),
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
        /// The index of the currently selected issue (0-indexed).
        selected: usize,
    },
    /// An error occurred.
    Error {
        /// The error message.
        message: String,
    },
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

/// The main application state container.
///
/// Manages transitions between the menu and views, and navigation within a loaded
/// view's issues.
pub struct App {
    /// The current screen.
    pub screen: Screen,
}

impl App {
    /// Creates a new application on the menu, first option highlighted.
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
        }
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
        let (kind, available) = MENU_OPTIONS[*selected];
        if !available {
            return None;
        }
        self.screen = Screen::View(kind, ViewState::Loading);
        Some(Action::EnterView)
    }

    /// Returns to the menu, selection reset to the first option.
    pub fn return_to_menu(&mut self) {
        self.screen = Screen::Menu { selected: 0 };
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
    /// issue. No-op if not currently in a view.
    pub fn set_issues(&mut self, issues: Vec<Issue>) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(kind, ViewState::Loaded { issues, selected: 0 });
        }
    }

    /// Transitions the current view to an error state with the given message.
    /// No-op if not currently in a view.
    pub fn set_error(&mut self, message: String) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(kind, ViewState::Error { message });
        }
    }

    /// Transitions the current view back to its loading state. No-op if not
    /// currently in a view.
    pub fn retry(&mut self) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(kind, ViewState::Loading);
        }
    }

    /// Moves the selection down one position if there are more issues below.
    /// No-op outside a loaded view.
    pub fn move_selection_down(&mut self) {
        if let Screen::View(_, ViewState::Loaded { issues, selected }) = &mut self.screen {
            if !issues.is_empty() && *selected + 1 < issues.len() {
                *selected += 1;
            }
        }
    }

    /// Moves the selection up one position if there are issues above. No-op
    /// outside a loaded view.
    pub fn move_selection_up(&mut self) {
        if let Screen::View(
            _,
            ViewState::Loaded {
                issues: _,
                selected,
            },
        ) = &mut self.screen
        {
            if *selected > 0 {
                *selected -= 1;
            }
        }
    }

    /// Returns a reference to the currently selected issue, if any.
    pub fn selected_issue(&self) -> Option<&Issue> {
        match &self.screen {
            Screen::View(_, ViewState::Loaded { issues, selected }) => issues.get(*selected),
            _ => None,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    OpenInBrowser(String),
    Retry,
    /// A menu option was entered; the caller should trigger a data fetch for the
    /// now-current view (see [`App::current_view`]).
    EnterView,
}

/// Map a key press to an [`Action`], applying any state change (menu navigation,
/// entering a view, list navigation, retry, returning to the menu) directly to
/// `app`. Returns `None` when the key had no effect or only changed state in place.
pub fn handle_key(app: &mut App, key: crossterm::event::KeyCode) -> Option<Action> {
    use crossterm::event::KeyCode;

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

    match key {
        KeyCode::Char('q') => Some(Action::Quit),
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
        KeyCode::Char('r') => {
            if app.is_view_error() {
                app.retry();
                Some(Action::Retry)
            } else {
                None
            }
        }
        _ => None,
    }
}
```

- [ ] **Step 4: Replace `src/plugin/ui.rs` in full**

Still expected to fail to compile after this step in isolation from Step 1-3 being present, but combined with Step 3 the crate should now compile — this step is what makes it compile, since `ui.rs` is also part of the `--lib` build and still references the old `AppState`.

Replace the entire file with:

```rust
//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{App, Screen, ViewState, MENU_OPTIONS};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(_, view_state) => draw_view(frame, view_state),
    }
}

fn draw_menu(frame: &mut Frame, selected: usize) {
    let items: Vec<ListItem> = MENU_OPTIONS
        .iter()
        .map(|(kind, available)| {
            let label = if *available {
                kind.label().to_string()
            } else {
                format!("{} (coming soon)", kind.label())
            };
            let style = if *available {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            ListItem::new(label).style(style)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Linear"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut list_state = ListState::default();
    list_state.select(Some(selected));
    frame.render_stateful_widget(list, frame.area(), &mut list_state);
}

fn draw_view(frame: &mut Frame, view_state: &ViewState) {
    match view_state {
        ViewState::Loading => {
            let paragraph = Paragraph::new("Loading issues...")
                .block(Block::default().borders(Borders::ALL).title("Linear"));
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Error { message } => {
            let paragraph = Paragraph::new(format!("{message}\n\nPress r to retry."))
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Linear - Error"),
                );
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Loaded { issues, selected } => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(frame.area());

            let items: Vec<ListItem> = issues
                .iter()
                .map(|issue| ListItem::new(format!("{} {}", issue.identifier, issue.title)))
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("My Issues"))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            let mut list_state = ListState::default();
            list_state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[0], &mut list_state);

            let detail = issues
                .get(*selected)
                .map(|issue| {
                    format!(
                        "{}\n\n{}\n\nState: {}\nURL: {}",
                        issue.identifier, issue.title, issue.state.name, issue.url
                    )
                })
                .unwrap_or_default();
            let detail_widget = Paragraph::new(detail)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title("Detail"));
            frame.render_widget(detail_widget, chunks[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::app::App;
    use crate::Issue;
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;

    fn sample_issue(identifier: &str) -> Issue {
        serde_json::from_value(json!({
            "id": format!("issue-{identifier}"),
            "identifier": identifier,
            "title": format!("Title for {identifier}"),
            "description": null,
            "state": {"id": "state-1", "name": "In Progress", "type": "started"},
            "priority": 2,
            "estimate": null,
            "team": {
                "id": "team-1", "key": "ENG", "name": "Engineering",
                "description": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "assignee": null,
            "creator": {
                "id": "user-1", "email": "a@example.com", "name": "Alice",
                "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "startedAt": null,
            "completedAt": null,
            "cycle": null,
            "project": null,
            "labels": {"nodes": []},
            "url": format!("https://linear.app/team/issue/{identifier}")
        }))
        .expect("valid issue payload")
    }

    fn rendered_text(app: &App) -> String {
        let backend = TestBackend::new(60, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// An `App` that has already entered the "My Issues" view (still `Loading`).
    fn app_in_my_issues_view() -> App {
        let mut app = App::new();
        app.enter_selected_menu_option();
        app
    }

    #[test]
    fn renders_all_three_menu_options_on_start() {
        let app = App::new();
        let text = rendered_text(&app);

        assert!(text.contains("My Issues"));
        assert!(text.contains("Project Issues"));
        assert!(text.contains("Team Issues"));
    }

    #[test]
    fn marks_unavailable_menu_options_as_coming_soon() {
        let app = App::new();
        let text = rendered_text(&app);

        assert!(text.contains("Project Issues (coming soon)"));
        assert!(text.contains("Team Issues (coming soon)"));
        assert!(!text.contains("My Issues (coming soon)"));
    }

    #[test]
    fn renders_loading_message() {
        let app = app_in_my_issues_view();
        assert!(rendered_text(&app).contains("Loading"));
    }

    #[test]
    fn renders_error_message_with_retry_hint() {
        let mut app = app_in_my_issues_view();
        app.set_error("Authentication failed".to_string());

        let text = rendered_text(&app);
        assert!(text.contains("Authentication failed"));
        assert!(text.contains("retry"));
    }

    #[test]
    fn renders_issue_identifier_and_title_in_the_list() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1"));
        assert!(text.contains("Title for ENG-1"));
    }
}
```

- [ ] **Step 5: Run the lib test suite and verify everything passes**

Run: `cargo test --lib --features plugin -- --nocapture`
Expected: PASS — all tests in `plugin::app::tests` and `plugin::ui::tests` green (25 tests in `app.rs`, 5 in `ui.rs`), plus every other existing lib test unaffected.

- [ ] **Step 6: Format and lint**

Run: `just fmt && just lint`
Expected: `fmt` makes no further changes (or auto-formats cleanly); `lint` (`cargo clippy --all-targets --all-features -- -D warnings`) reports no warnings. Fix anything it flags before continuing — do not silence with `#[allow(...)]` unless the lint is a genuine false positive.

- [ ] **Step 7: Commit**

```bash
git add src/plugin/app.rs src/plugin/ui.rs
git commit -m "feat: add a menu-first view switcher to the plugin state machine

Nests the existing Loading/Loaded/Error state machine (renamed
ViewState) inside a new Screen enum (Menu vs View), so the plugin
no longer jumps straight into My Issues. Project Issues and Team
Issues are visible in the menu but disabled until TF-578/TF-579
implement their data fetching."
```

---

### Task 2: Wire `main.rs`'s event loop to the menu-first `App`

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes (from Task 1): `plugin::app::{App, ViewKind, Action}`, `App::current_view()`, `App::set_issues()`, `App::set_error()`, `handle_key()` returning `Option<Action>` including the new `Action::EnterView`.

- [ ] **Step 1: Replace `load_issues`, `ensure_loaded`, and `event_loop` in `src/main.rs`**

Replace the three functions (from `async fn load_issues` through the end of `async fn event_loop`, i.e. everything between `run_tui`'s closing brace and the `#[cfg(test)]` module) with:

```rust
async fn load_issues(app: &mut plugin::app::App, client: &herdr_linear::LinearClient) {
    match app.current_view() {
        Some(plugin::app::ViewKind::MyIssues) => {
            match plugin::data::fetch_my_issues(client).await {
                Ok(issues) => app.set_issues(issues),
                Err(err) => app.set_error(err.to_string()),
            }
        }
        Some(plugin::app::ViewKind::ProjectIssues) | Some(plugin::app::ViewKind::TeamIssues) => {
            unreachable!("the menu does not allow selecting an unavailable view yet")
        }
        None => {}
    }
}

/// Build the `LinearClient` if it doesn't exist yet (resolving config, then
/// constructing the client), then fetch issues for the currently entered view
/// through it. On a config/client failure, sets an inline error on `app` instead of
/// propagating — this is what lets a missing/invalid API key show up in the TUI
/// rather than crashing the process, and lets `r` (retry) recover from a config
/// typo without a restart.
async fn ensure_loaded(
    app: &mut plugin::app::App,
    client: &mut Option<herdr_linear::LinearClient>,
) {
    if client.is_none() {
        match plugin::config::load().and_then(herdr_linear::LinearClient::new) {
            Ok(c) => *client = Some(c),
            Err(err) => {
                app.set_error(err.to_string());
                return;
            }
        }
    }

    if let Some(c) = client.as_ref() {
        load_issues(app, c).await;
    }
}

async fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut plugin::app::App,
    client: &mut Option<herdr_linear::LinearClient>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|frame| plugin::ui::draw(frame, app))?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if let Some(action) = plugin::app::handle_key(app, key.code) {
                    match action {
                        plugin::app::Action::Quit => break,
                        plugin::app::Action::OpenInBrowser(url) => {
                            let _ = open::that(url);
                        }
                        plugin::app::Action::Retry | plugin::app::Action::EnterView => {
                            // `handle_key` already moved `app` into `Loading` — either
                            // retrying the current view or entering a newly selected
                            // one; draw that before the fetch's own round-trip so
                            // it's visible instead of leaving the stale previous frame.
                            terminal.draw(|frame| plugin::ui::draw(frame, app))?;
                            ensure_loaded(app, client).await;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
```

Note what's removed versus today: the pre-loop `terminal.draw(...)` + `ensure_loaded(app, client).await` pair (and its doc comment) that used to run before the `loop {`. It's no longer correct — the app now starts on the menu, which needs no network call, and the loop's own `terminal.draw` at the top already renders the menu on the first iteration.

- [ ] **Step 2: Build the plugin binary**

Run: `cargo build --features plugin`
Expected: builds successfully with no errors.

- [ ] **Step 3: Run the full quality gate**

Run: `just check`
Expected: `fmt`, `lint` (clippy `-D warnings`), and `test` (`cargo test --all-features -- --nocapture`, covering both the lib and the `herdr-linear` binary's own `dispatch_launch_decision` tests) all pass, ending with `✅ All checks passed!`.

- [ ] **Step 4: Manual smoke test**

Automated tests don't drive a real terminal, so confirm the TUI behavior by hand (same verification approach used for the original plugin layer — see `docs/superpowers/specs/2026-08-04-herdr-plugin-layer-design.md`'s Testing section):

```bash
LINEAR_API_KEY=<your key> cargo run --features plugin
```

Walk through and confirm:
1. The app opens showing the menu with three entries: "My Issues", "Project Issues (coming soon)", "Team Issues (coming soon)".
2. ↓ / ↑ moves the highlight between all three entries, clamped at the top and bottom (doesn't wrap).
3. Pressing Enter while "Project Issues" or "Team Issues" is highlighted does nothing (still on the menu).
4. Pressing Enter on "My Issues" shows "Loading issues...", then the existing two-pane issue list once it loads.
5. Pressing Esc from the loaded list returns to the menu (not quit).
6. Selecting "My Issues" again from the menu re-loads issues correctly.
7. Pressing `q` from both the menu and the loaded list quits the app.
8. With an invalid `LINEAR_API_KEY`, entering "My Issues" shows the error screen with the retry hint, `r` retries, and `Esc` from the error screen also returns to the menu.

Press `Ctrl+C` if anything hangs — the panic hook and teardown in `run_tui`/`main` are unchanged by this plan, so the terminal should always be restored on exit.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: drive the plugin event loop from the new view-switcher menu

Starts the app on the menu instead of immediately loading My
Issues; entering an available view now triggers the same
load-then-render sequence retry already used, via the new
Action::EnterView."
```

---

## After this plan

- Mark [TF-576](https://linear.app/talent-factory/issue/TF-576) as Done in Linear.
- Tick the "View switcher: My Issues / Project Issues / Team Issues" checkbox under Phase 1.6 in `ROADMAP.md`.
- TF-578 ("Project Issues" view) and TF-579 ("Team Issues" view) are now unblocked — they only need to add a `fetch_*` function in `plugin::data` and flip their `bool` in `MENU_OPTIONS` to `true`.
