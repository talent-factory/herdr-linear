//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{App, Screen, Status, ViewKind, ViewState, MENU_OPTIONS};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
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

/// `tui-markdown`'s default `StyleSheet` renders heading markers and code
/// fences literally (e.g. `"# Heading"`, a bare ` ``` ` line around a code
/// block), which reads as raw Markdown syntax rather than distinctly
/// formatted content. This override drops both markers; a heading stays
/// visually distinct via the unmodified `StyleSheet::heading` (bold and
/// underlined for H1, lighter treatments for lower levels), and code block
/// content is still rendered via `StyleSheet::code` — only the surrounding
/// fence line disappears.
#[derive(Clone)]
struct MarkdownStyleSheet;

impl tui_markdown::StyleSheet for MarkdownStyleSheet {
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code_block_fence(&self) -> &str {
        ""
    }
}

fn draw_view(frame: &mut Frame, kind: ViewKind, view_state: &ViewState, status: Option<&Status>) {
    match view_state {
        ViewState::Loading => {
            let paragraph = Paragraph::new("Loading issues...")
                .block(Block::default().borders(Borders::ALL).title("Linear"));
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Error { message } => {
            let paragraph = Paragraph::new(format!(
                "{message}\n\nPress c to edit config.toml · Press r to retry."
            ))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Linear - Error"),
            );
            frame.render_widget(paragraph, frame.area());
        }
        ViewState::Loaded { issues, selected } => {
            let area = if let Some(status) = status {
                let outer = Layout::default()
                    .direction(Direction::Vertical)
                    // 3 rows (not 1) so a long error message — these can nest a whole
                    // underlying `herdr`/Linear error plus a manual-fallback prompt — wraps
                    // instead of being silently truncated at terminal width.
                    .constraints([Constraint::Min(3), Constraint::Length(3)])
                    .split(frame.area());
                let style = if status.is_error() {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Green)
                };
                frame.render_widget(
                    Paragraph::new(status.text())
                        .style(style)
                        .wrap(Wrap { trim: false }),
                    outer[1],
                );
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

            let detail_block = Block::default().borders(Borders::ALL).title("Detail");
            let detail_area = detail_block.inner(chunks[1]);
            frame.render_widget(detail_block, chunks[1]);

            if let Some(issue) = issues.get(*selected) {
                // Header (identifier/title/Status/Assignee/Project) and body
                // (the Markdown description) are rendered as one continuous
                // `Text` in a single `Min(0)` area rather than two areas split
                // by a fixed-height header. A fixed header height clips
                // whatever doesn't fit once a long title wraps past one line —
                // exactly what happened to `State:` before this change — and
                // there's no reliable way to pre-compute the wrapped height
                // without depending on ratatui's unstable line-counting API.
                // A single scrollable block sidesteps that entirely: nothing
                // downstream of the title can be clipped by it.
                let assignee = issue
                    .assignee
                    .as_ref()
                    .map(|user| user.name.as_str())
                    .unwrap_or("Unassigned");
                let project = issue
                    .project
                    .as_ref()
                    .map(|project| project.name.as_str())
                    .unwrap_or("None");

                let mut lines = vec![
                    Line::from(issue.identifier.as_str()),
                    Line::from(""),
                    Line::from(issue.title.as_str()),
                    Line::from(""),
                    Line::from(format!("Status: {}", issue.state.name)),
                    Line::from(format!("Assignee: {assignee}")),
                    Line::from(format!("Project: {project}")),
                    Line::from(""),
                ];

                let description = issue.description.as_deref().unwrap_or_default();
                let options = tui_markdown::Options::new(MarkdownStyleSheet);
                lines.extend(tui_markdown::from_str_with_options(description, &options).lines);

                let sections = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(1)])
                    .split(detail_area);

                // `trim: false` preserves each line's leading whitespace, which
                // is exactly what Markdown uses to convey nested lists and
                // indented code — `trim: true` would strip it and flatten that
                // structure away. The header lines above have no leading
                // whitespace to begin with, so this is harmless for them.
                let body = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
                frame.render_widget(body, sections[0]);

                let footer = Paragraph::new(format!("URL: {}", issue.url));
                frame.render_widget(footer, sections[1]);
            }
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

    #[allow(clippy::too_many_arguments)]
    fn sample_issue_json(
        identifier: &str,
        description: Option<&str>,
        assignee_name: Option<&str>,
        project_name: Option<&str>,
    ) -> serde_json::Value {
        let assignee = assignee_name.map(|name| {
            json!({
                "id": "user-2", "email": "b@example.com", "name": name,
                "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            })
        });
        let project = project_name.map(|name| {
            json!({
                "id": "project-1", "name": name, "description": null,
                "url": "https://linear.app/team/project/proj-1",
                "leadId": null, "lead": null,
                "status": {"id": "status-1", "name": "Planned", "type": "planned"},
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z",
                "startDate": null, "targetDate": null
            })
        });

        json!({
            "id": format!("issue-{identifier}"),
            "identifier": identifier,
            "title": format!("Title for {identifier}"),
            "description": description,
            "state": {"id": "state-1", "name": "In Progress", "type": "started"},
            "priority": 2,
            "estimate": null,
            "team": {
                "id": "team-1", "key": "ENG", "name": "Engineering",
                "description": null,
                "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            },
            "assignee": assignee,
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
            "project": project,
            "labels": {"nodes": []},
            "url": format!("https://linear.app/team/issue/{identifier}")
        })
    }

    fn sample_issue(identifier: &str) -> Issue {
        serde_json::from_value(sample_issue_json(identifier, None, None, None))
            .expect("valid issue payload")
    }

    fn sample_issue_with_description(identifier: &str, description: &str) -> Issue {
        serde_json::from_value(sample_issue_json(identifier, Some(description), None, None))
            .expect("valid issue payload")
    }

    fn sample_issue_with_metadata(identifier: &str, assignee: &str, project: &str) -> Issue {
        serde_json::from_value(sample_issue_json(
            identifier,
            None,
            Some(assignee),
            Some(project),
        ))
        .expect("valid issue payload")
    }

    fn rendered_text_with_size(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
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

    fn rendered_text(app: &App) -> String {
        rendered_text_with_size(app, 60, 15)
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
        app.set_status(Status::Ok(
            "ENG-1: tab opened, agent started, set to In Progress.".to_string(),
        ));

        let text = rendered_text(&app);
        assert!(text.contains("ENG-1: tab opened, agent started, set to In Progress."));
    }

    #[test]
    fn renders_an_error_status_banner() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_status(Status::Error(
            "ENG-1: failed to start agent tab: boom".to_string(),
        ));

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

    #[test]
    fn renders_issue_description_as_formatted_markdown() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-2",
            "# Heading\n\n- item one\n- item two\n\n**bold** and `code`\n\n\
             ```rust\nlet answer = 42;\n```",
        )]);

        let text = rendered_text_with_size(&app, 100, 30);
        assert!(text.contains("Heading"));
        assert!(!text.contains("# Heading"));
        assert!(text.contains("item one"));
        assert!(text.contains("item two"));
        assert!(text.contains("bold"));
        assert!(!text.contains("**bold**"));
        assert!(text.contains("code"));
        assert!(!text.contains("`code`"));
        // Regression: fenced code blocks previously showed the ``` fence
        // markers literally instead of just the code content.
        assert!(text.contains("let answer = 42;"));
        assert!(!text.contains("```"));
    }

    #[test]
    fn renders_issue_url_in_the_detail_footer() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-3")]);

        let text = rendered_text_with_size(&app, 100, 20);
        assert!(text.contains("URL: https://linear.app/team/issue/ENG-3"));
    }

    #[test]
    fn renders_status_assignee_and_project_in_the_detail_pane() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_metadata(
            "ENG-4",
            "Alice",
            "Herdr Linear",
        )]);

        let text = rendered_text_with_size(&app, 100, 20);
        assert!(text.contains("Status: In Progress"));
        assert!(text.contains("Assignee: Alice"));
        assert!(text.contains("Project: Herdr Linear"));
    }

    #[test]
    fn renders_unassigned_and_no_project_fallbacks_when_absent() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-5")]);

        let text = rendered_text_with_size(&app, 100, 20);
        assert!(text.contains("Assignee: Unassigned"));
        assert!(text.contains("Project: None"));
    }

    #[test]
    fn does_not_clip_status_assignee_or_project_behind_a_long_wrapped_title() {
        // Regression: a fixed-height header (`Constraint::Length(5)`) clipped
        // `State:` (now `Status:`) once a long title wrapped past a single
        // line — exactly the scenario a wide-but-long title forces here. The
        // terminal is wide enough that "Status: In Progress" etc. don't wrap
        // themselves (`rendered_text_with_size` flattens rows without line
        // breaks, so a wrapped multi-line match wouldn't `contains()` cleanly —
        // that's a test-harness limitation, not a rendering one), but narrow
        // enough that the long title still wraps across several lines.
        let mut app = app_in_my_issues_view();
        let mut issue = sample_issue_with_metadata("ENG-6", "Alice", "Herdr Linear");
        issue.title =
            "A deliberately long issue title that will wrap across several lines in a narrow pane"
                .to_string();
        app.set_issues(vec![issue]);

        let text = rendered_text_with_size(&app, 60, 20);
        assert!(text.contains("Status: In Progress"));
        assert!(text.contains("Assignee: Alice"));
        assert!(text.contains("Project: Herdr Linear"));
    }
}
