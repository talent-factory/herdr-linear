//! Rendering for the plugin TUI: a loading message, an error message with a retry
//! hint, or a two-pane issue list + detail view.

use crate::plugin::app::{App, AppState};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &App) {
    match &app.state {
        AppState::Loading => {
            let paragraph = Paragraph::new("Loading issues...")
                .block(Block::default().borders(Borders::ALL).title("Linear"));
            frame.render_widget(paragraph, frame.area());
        }
        AppState::Error { message } => {
            let paragraph = Paragraph::new(format!("{message}\n\nPress r to retry."))
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Linear - Error"),
                );
            frame.render_widget(paragraph, frame.area());
        }
        AppState::Loaded { issues, selected } => {
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
                "description": null, "avatarUrl": null,
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
            "labels": [],
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

    #[test]
    fn renders_loading_message() {
        let app = App::new();
        assert!(rendered_text(&app).contains("Loading"));
    }

    #[test]
    fn renders_error_message_with_retry_hint() {
        let mut app = App::new();
        app.set_error("Authentication failed".to_string());

        let text = rendered_text(&app);
        assert!(text.contains("Authentication failed"));
        assert!(text.contains("retry"));
    }

    #[test]
    fn renders_issue_identifier_and_title_in_the_list() {
        let mut app = App::new();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1"));
        assert!(text.contains("Title for ENG-1"));
    }
}
