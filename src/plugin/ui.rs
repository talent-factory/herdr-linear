//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{App, Screen, ViewKind, ViewState, MENU_OPTIONS};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => draw_view(frame, *kind, view_state, app.status()),
    }
}

fn draw_menu(frame: &mut Frame, selected: usize) {
    let items: Vec<ListItem> = MENU_OPTIONS
        .iter()
        .map(|option| {
            let label = if option.available {
                option.kind.label().to_string()
            } else {
                format!("{} (coming soon)", option.kind.label())
            };
            let style = if option.available {
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

fn draw_view(
    frame: &mut Frame,
    kind: ViewKind,
    view_state: &ViewState,
    status: Option<(&str, bool)>,
) {
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
            let area = if let Some((text, is_error)) = status {
                let outer = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(1)])
                    .split(frame.area());
                let style = if is_error {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                frame.render_widget(Paragraph::new(text).style(style), outer[1]);
                outer[0]
            } else {
                frame.area()
            };

            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);

            let items: Vec<ListItem> = issues
                .iter()
                .map(|issue| ListItem::new(format!("{} {}", issue.identifier, issue.title)))
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(kind.label()))
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

        assert!(text.contains("Team Issues (coming soon)"));
        assert!(!text.contains("My Issues (coming soon)"));
        assert!(!text.contains("Project Issues (coming soon)"));
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

    #[test]
    fn renders_the_status_banner_when_present() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status("ENG-1: tab opened, agent started, set to In Progress.".to_string(), false);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1: tab opened, agent started, set to In Progress."));
    }

    #[test]
    fn renders_an_error_status_banner() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status("ENG-1: failed to start agent tab: boom".to_string(), true);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1: failed to start agent tab: boom"));
    }

    #[test]
    fn renders_without_a_status_banner_when_none_is_set() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1"));
        assert!(text.contains("Title for ENG-1"));
    }
}
