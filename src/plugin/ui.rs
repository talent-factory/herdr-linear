//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{App, Screen, ViewKind, ViewState, MENU_OPTIONS};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Widget, Wrap},
    Frame,
};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => draw_view(frame, *kind, view_state),
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

/// `tui-markdown`'s default `StyleSheet` renders heading markers literally
/// (e.g. `"# Heading"`), which reads as raw Markdown syntax rather than a
/// distinctly-formatted heading. This override drops the marker; the heading
/// is still visually distinct via `StyleSheet::heading`'s bold/underline
/// style (unchanged from the default).
#[derive(Clone)]
struct MarkdownStyleSheet;

impl tui_markdown::StyleSheet for MarkdownStyleSheet {
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }
}

/// A single-line widget that renders `text` normally, then wraps the trailing
/// `url.chars().count()` cells — the URL substring at the end of `text`, e.g.
/// `"URL: {url}"` — in an OSC 8 terminal hyperlink escape sequence. The escape
/// bytes are zero-width to the terminal, so layout/wrapping is unaffected, and
/// terminals without OSC 8 support just show the plain text.
struct Hyperlink<'a> {
    text: Line<'a>,
    url: String,
}

impl<'a> Hyperlink<'a> {
    fn new(text: Line<'a>, url: impl Into<String>) -> Self {
        Self {
            text,
            url: url.into(),
        }
    }
}

impl Widget for &Hyperlink<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.text.clone()).render(area, buf);

        let link_width = self.url.chars().count();
        let total_width = self.text.width();
        let start = total_width
            .saturating_sub(link_width)
            .min(area.width as usize);
        let end = total_width.min(area.width as usize);

        for x in start..end {
            let Some(cell) = buf.cell_mut((area.x + x as u16, area.y)) else {
                continue;
            };
            let symbol = cell.symbol().to_string();
            let wrapped = match (x == start, x + 1 == end) {
                (true, true) => format!("\x1b]8;;{}\x1b\\{symbol}\x1b]8;;\x1b\\", self.url),
                (true, false) => format!("\x1b]8;;{}\x1b\\{symbol}", self.url),
                (false, true) => format!("{symbol}\x1b]8;;\x1b\\"),
                (false, false) => symbol,
            };
            cell.set_symbol(&wrapped);
        }
    }
}

fn draw_view(frame: &mut Frame, kind: ViewKind, view_state: &ViewState) {
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
                .block(Block::default().borders(Borders::ALL).title(kind.label()))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            let mut list_state = ListState::default();
            list_state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[0], &mut list_state);

            let detail_block = Block::default().borders(Borders::ALL).title("Detail");
            let detail_area = detail_block.inner(chunks[1]);
            frame.render_widget(detail_block, chunks[1]);

            if let Some(issue) = issues.get(*selected) {
                let sections = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(5),
                        Constraint::Min(0),
                        Constraint::Length(1),
                    ])
                    .split(detail_area);

                let header = Paragraph::new(format!(
                    "{}\n\n{}\n\nState: {}",
                    issue.identifier, issue.title, issue.state.name
                ))
                .wrap(Wrap { trim: true });
                frame.render_widget(header, sections[0]);

                let description = issue.description.as_deref().unwrap_or_default();
                let options = tui_markdown::Options::new(MarkdownStyleSheet);
                let body = Paragraph::new(tui_markdown::from_str_with_options(
                    description,
                    &options,
                ))
                .wrap(Wrap { trim: true });
                frame.render_widget(body, sections[1]);

                let hyperlink =
                    Hyperlink::new(Line::from(format!("URL: {}", issue.url)), issue.url.clone());
                frame.render_widget(&hyperlink, sections[2]);
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

    fn sample_issue_json(identifier: &str, description: Option<&str>) -> serde_json::Value {
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
        })
    }

    fn sample_issue(identifier: &str) -> Issue {
        serde_json::from_value(sample_issue_json(identifier, None)).expect("valid issue payload")
    }

    fn sample_issue_with_description(identifier: &str, description: &str) -> Issue {
        serde_json::from_value(sample_issue_json(identifier, Some(description)))
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
    fn hyperlink_wraps_the_trailing_url_portion_in_osc8_escapes() {
        let url = "https://example.com";
        let hyperlink = Hyperlink::new(Line::from(format!("URL: {url}")), url);
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&hyperlink, area, &mut buf);

        let start = "URL: ".chars().count(); // 5
        let url_len = url.chars().count(); // 20
        let open = format!("\x1b]8;;{url}\x1b\\");
        let close = "\x1b]8;;\x1b\\";

        let first = buf.cell((start as u16, 0)).unwrap().symbol().to_string();
        let last = buf
            .cell(((start + url_len - 1) as u16, 0))
            .unwrap()
            .symbol()
            .to_string();
        let middle = buf
            .cell(((start + 1) as u16, 0))
            .unwrap()
            .symbol()
            .to_string();

        assert!(first.starts_with(&open), "first cell was {first:?}");
        assert!(last.ends_with(close), "last cell was {last:?}");
        assert_eq!(middle, "t"); // second char of "https://..."

        // Text before the link (the "URL: " label) is untouched.
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "U");
        assert_eq!(buf.cell((4, 0)).unwrap().symbol(), " ");
    }

    #[test]
    fn hyperlink_wraps_open_and_close_in_the_same_cell_for_a_single_character_link() {
        let hyperlink = Hyperlink::new(Line::from("x"), "x");
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&hyperlink, area, &mut buf);

        let cell = buf.cell((0, 0)).unwrap().symbol().to_string();
        assert_eq!(cell, "\x1b]8;;x\x1b\\x\x1b]8;;\x1b\\");
    }

    #[test]
    fn renders_issue_description_as_formatted_markdown() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-2",
            "# Heading\n\n- item one\n- item two\n\n**bold** and `code`",
        )]);

        let text = rendered_text(&app);
        assert!(text.contains("Heading"));
        assert!(!text.contains("# Heading"));
        assert!(text.contains("item one"));
        assert!(text.contains("item two"));
        assert!(text.contains("bold"));
        assert!(!text.contains("**bold**"));
        assert!(text.contains("code"));
        assert!(!text.contains("`code`"));
    }

    #[test]
    fn renders_issue_url_in_the_detail_footer() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-3")]);

        let text = rendered_text_with_size(&app, 100, 20);
        // The URL is OSC 8-wrapped by `Hyperlink` (covered separately by the
        // `hyperlink_*` tests above), so "URL: " and the URL text are no
        // longer one contiguous substring — check both are present instead.
        assert!(text.contains("URL: "));
        assert!(text.contains("https://linear.app/team/issue/ENG-3"));
    }
}
