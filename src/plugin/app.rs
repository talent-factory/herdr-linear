//! TUI application state and navigation.
//!
//! Provides pure state management for the terminal UI without any rendering logic.
//! The app starts on a menu (`Screen::Menu`) listing the configured issue views —
//! unavailable ones are shown but disabled — then moves into `Screen::View` once an
//! available one is selected, tracking that view's own display state (loading,
//! loaded issues, error) and navigation within its issue list.

use crate::Issue;

/// The views selectable from the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// Issues assigned to the authenticated user.
    MyIssues,
    /// All open issues in the current project.
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

/// A single entry in the view-selection menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuOption {
    /// The view this entry selects.
    pub kind: ViewKind,
    /// Whether this entry is selectable yet.
    pub available: bool,
}

/// The menu options in display order. `TeamIssues` becomes available once TF-579
/// implements its data fetching — until then the menu shows but disables it.
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
        available: false,
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

/// The main application state container.
///
/// Manages transitions between the menu and views, and navigation within a loaded
/// view's issues.
pub struct App {
    /// The current screen.
    screen: Screen,
    /// The current status banner, if any. See [`Status`].
    status: Option<Status>,
}

impl App {
    /// Creates a new application on the menu, first option highlighted.
    pub fn new() -> Self {
        Self {
            screen: Screen::Menu { selected: 0 },
            status: None,
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
    /// issue. No-op if not currently in a view.
    pub fn set_issues(&mut self, issues: Vec<Issue>) {
        if let Some(kind) = self.current_view() {
            self.screen = Screen::View(
                kind,
                ViewState::Loaded {
                    issues,
                    selected: 0,
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
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
// `Issue` is the largest variant by far; boxing it would ripple into every call site that
// constructs or matches `Action::Implement`, and the plan locks this interface to a bare
// `Issue` for Task 5/6 to consume.
#[allow(clippy::large_enum_variant)]
pub enum Action {
    Quit,
    OpenInBrowser(String),
    Retry,
    /// A menu option was entered; the caller should trigger a data fetch for the
    /// now-current view (see [`App::current_view`]).
    EnterView,
    /// `<Enter>` was pressed on a selected issue: open a herdr tab, start the preferred
    /// coding agent, set the issue to "In Progress", and inject the implement prompt once
    /// ready. Orchestrated in `main.rs`'s `start_implementation`.
    Implement(Issue),
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
        KeyCode::Enter => app
            .selected_issue()
            .map(|issue| Action::Implement(issue.clone())),
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
    fn entering_an_unavailable_option_does_nothing() {
        let mut app = App::new();
        app.move_menu_selection_down();
        app.move_menu_selection_down(); // -> Team Issues, unavailable

        let action = app.enter_selected_menu_option();

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 2 }));
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
    fn enter_key_returns_implement_action_with_the_selected_issue() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        let action = handle_key(&mut app, KeyCode::Enter);

        assert_eq!(action, Some(Action::Implement(sample_issue("ENG-1"))));
    }

    #[test]
    fn enter_key_in_a_view_on_an_empty_list_returns_no_action() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![]);

        assert_eq!(handle_key(&mut app, KeyCode::Enter), None);
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

        handle_key(&mut app, KeyCode::Esc);

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
        handle_key(&mut app, KeyCode::Down);
        handle_key(&mut app, KeyCode::Down); // -> Team Issues, unavailable

        let action = handle_key(&mut app, KeyCode::Enter);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 2 }));
    }

    #[test]
    fn up_key_from_the_menu_moves_selection_and_returns_no_action() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Down);

        let action = handle_key(&mut app, KeyCode::Up);

        assert_eq!(action, None);
        assert!(matches!(app.screen, Screen::Menu { selected: 0 }));
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
    fn reentering_a_view_after_esc_starts_a_fresh_loading_state() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Esc);

        let action = handle_key(&mut app, KeyCode::Enter);

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
