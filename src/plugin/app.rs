//! TUI application state and navigation.
//!
//! Provides pure state management for the terminal UI without any rendering logic.
//! Tracks the current display state (loading, loaded issues, error) and handles
//! navigation between issues in the list.

use crate::Issue;

/// The application state, representing what the UI should currently display.
///
/// Note: `AppState` deliberately does NOT derive `PartialEq` because `Issue`
/// doesn't derive it either. Tests use `matches!` for state comparisons instead.
#[derive(Debug)]
pub enum AppState {
    /// The application is loading issues.
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

/// The main application state container.
///
/// Manages transitions between states and navigation within the loaded issues.
pub struct App {
    /// The current application state.
    pub state: AppState,
}

impl App {
    /// Creates a new application in the loading state.
    pub fn new() -> Self {
        Self {
            state: AppState::Loading,
        }
    }

    /// Sets the loaded issues and resets selection to the first issue.
    pub fn set_issues(&mut self, issues: Vec<Issue>) {
        self.state = AppState::Loaded {
            issues,
            selected: 0,
        };
    }

    /// Transitions to an error state with the given message.
    pub fn set_error(&mut self, message: String) {
        self.state = AppState::Error { message };
    }

    /// Transitions back to the loading state.
    pub fn retry(&mut self) {
        self.state = AppState::Loading;
    }

    /// Moves the selection down one position if there are more issues below.
    pub fn move_selection_down(&mut self) {
        if let AppState::Loaded { issues, selected } = &mut self.state {
            if !issues.is_empty() && *selected + 1 < issues.len() {
                *selected += 1;
            }
        }
    }

    /// Moves the selection up one position if there are issues above.
    pub fn move_selection_up(&mut self) {
        if let AppState::Loaded {
            issues: _,
            selected,
        } = &mut self.state
        {
            if *selected > 0 {
                *selected -= 1;
            }
        }
    }

    /// Returns a reference to the currently selected issue, if any.
    pub fn selected_issue(&self) -> Option<&Issue> {
        match &self.state {
            AppState::Loaded { issues, selected } => issues.get(*selected),
            _ => None,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
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
                avatar_url: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            assignee: None,
            creator: crate::User {
                id: "user-id".to_string(),
                email: "test@example.com".to_string(),
                name: "Test User".to_string(),
                avatar_url: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            started_at: None,
            completed_at: None,
            cycle: None,
            project: None,
            labels: vec![],
            url: format!("https://linear.app/eng/{}", identifier),
        }
    }

    #[test]
    fn app_starts_in_loading_state() {
        let app = App::new();
        assert!(matches!(app.state, AppState::Loading));
    }

    #[test]
    fn set_issues_transitions_to_loaded_with_first_selected() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        assert!(matches!(&app.state, AppState::Loaded { .. }));
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn move_selection_down_advances_through_issues() {
        let mut app = App::new();
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
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);

        app.move_selection_down();
        app.move_selection_down();

        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-2");
    }

    #[test]
    fn move_selection_up_retreats_and_clamps_at_the_start() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");

        app.move_selection_up();
        assert_eq!(app.selected_issue().unwrap().identifier, "ENG-1");
    }

    #[test]
    fn navigation_on_an_empty_list_does_not_panic() {
        let mut app = App::new();
        app.set_issues(vec![]);

        app.move_selection_down();
        app.move_selection_up();

        assert!(app.selected_issue().is_none());
    }

    #[test]
    fn set_error_moves_to_error_state() {
        let mut app = App::new();
        app.set_error("boom".to_string());

        assert!(matches!(&app.state, AppState::Error { message } if message == "boom"));
    }

    #[test]
    fn retry_moves_back_to_loading() {
        let mut app = App::new();
        app.set_error("boom".to_string());

        app.retry();

        assert!(matches!(app.state, AppState::Loading));
    }
}
