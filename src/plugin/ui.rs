//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{
    matching_issue_indices, App, Screen, Status, ViewKind, ViewState, MENU_OPTIONS,
};
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

/// Floor and ceiling for [`status_banner_height`]'s estimate: never shrink the banner below
/// what a single short message needs (matches the fixed height this replaces), and never let a
/// pathological message (in principle bounded by `main.rs::MAX_STATUS_DETAILS`, but this is a
/// second, independent backstop) consume more than a third of a typical terminal.
const STATUS_BANNER_MIN_HEIGHT: u16 = 3;
const STATUS_BANNER_MAX_HEIGHT: u16 = 10;

/// How many rows to reserve for the status banner at the bottom of `draw_view`'s loaded-issue
/// layout, given the banner's `text` and the frame's `width`. Ratatui doesn't expose a stable
/// API to precompute a `Paragraph`'s exact wrapped height (the same reason the issue-detail
/// pane elsewhere in this module uses a single `Constraint::Min(0)` block instead of a fixed
/// header height — see its own comment), so this is a deliberately generous overestimate
/// (`ceil(chars / width) + 2` rows, clamped to `[STATUS_BANNER_MIN_HEIGHT,
/// STATUS_BANNER_MAX_HEIGHT]`) rather than an exact line count: erring high just leaves a
/// couple of blank trailing rows, erring low silently clips content — and a fixed `3` was sized
/// for one short message, not TF-590's multi-issue failure banner, which can carry one
/// `"<identifier>: <message>"` segment per failed issue.
fn status_banner_height(text: &str, width: u16) -> u16 {
    if width == 0 {
        return STATUS_BANNER_MIN_HEIGHT;
    }
    // `.min(u16::MAX as usize)` before the cast: `text` is in practice bounded by
    // `main.rs::MAX_STATUS_DETAILS`, but that's a count of *segments*, not a length cap on the
    // underlying `herdr`/Linear error text within each one — an unbounded single message could
    // otherwise wrap `as u16` around to a small number and *under*-estimate, which is exactly
    // the failure mode this function's own doc says it deliberately avoids.
    let chars = text.chars().count().min(u16::MAX as usize) as u16;
    let estimated = chars.div_ceil(width).saturating_add(2);
    estimated.clamp(STATUS_BANNER_MIN_HEIGHT, STATUS_BANNER_MAX_HEIGHT)
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
        ViewState::Loaded {
            issues,
            selected,
            marked,
            filter,
        } => {
            let area = if let Some(status) = status {
                let banner_height = status_banner_height(status.text(), frame.area().width);
                let outer = Layout::default()
                    .direction(Direction::Vertical)
                    // Sized from the message itself (see `status_banner_height`), not a fixed
                    // `3`, so a long error message — these can nest a whole underlying
                    // `herdr`/Linear error plus a manual-fallback prompt, or (TF-590) one
                    // segment per failed issue in a multi-issue run — wraps instead of being
                    // silently truncated at terminal width.
                    .constraints([Constraint::Min(3), Constraint::Length(banner_height)])
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

            // `selected` indexes this filtered subset, not `issues` directly — see
            // `matching_issue_indices`'s doc comment and `App::selected_issue`, which the
            // detail pane below mirrors exactly so the two panes never disagree about
            // which issue is highlighted.
            let matched_indices = matching_issue_indices(issues, &filter.query);

            // `▏` marks the live cursor position while editing, so it's visually
            // distinct from a confirmed-but-inactive filter shown without one.
            let list_title = match (filter.editing, filter.query.is_empty()) {
                (true, _) => format!("{} — filter: {}▏", kind.label(), filter.query),
                (false, true) => kind.label().to_string(),
                (false, false) => format!("{} — filter: {}", kind.label(), filter.query),
            };

            let items: Vec<ListItem> = if matched_indices.is_empty() && !issues.is_empty() {
                vec![
                    ListItem::new(format!("No issues match \"{}\"", filter.query))
                        .style(Style::default().add_modifier(Modifier::DIM)),
                ]
            } else {
                matched_indices
                    .iter()
                    .map(|&index| {
                        let issue = &issues[index];
                        // TF-590: a checkbox prefix makes multi-select marks (`<Space>`)
                        // visible in the list, not just implicit in `App`'s internal state.
                        // `marked` holds raw `issues` indices (see its doc comment), so this
                        // checks the same `index` the issue itself came from, not its
                        // position within the filtered `matched_indices` list.
                        let checkbox = if marked.contains(&index) {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        ListItem::new(format!("{checkbox} {} {}", issue.identifier, issue.title))
                    })
                    .collect()
            };
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(list_title))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
            let mut list_state = ListState::default();
            list_state.select(Some(*selected));
            frame.render_stateful_widget(list, chunks[0], &mut list_state);

            let detail_block = Block::default().borders(Borders::ALL).title("Detail");
            let detail_area = detail_block.inner(chunks[1]);
            frame.render_widget(detail_block, chunks[1]);

            let selected_issue = matched_indices
                .get(*selected)
                .and_then(|&index| issues.get(index));
            if let Some(issue) = selected_issue {
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
    use crate::plugin::app::{handle_key, App};
    use crate::Issue;
    use crossterm::event::{KeyCode, KeyModifiers};
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

    /// As of TF-579 all three menu options are available, so none should show the
    /// "(coming soon)" suffix `draw_menu` adds for `!option.available` entries —
    /// replaces the old `marks_unavailable_menu_options_as_coming_soon`, which
    /// asserted Team Issues showed it.
    #[test]
    fn no_menu_option_is_marked_coming_soon() {
        let app = App::new();
        let text = rendered_text(&app);

        assert!(!text.contains("(coming soon)"));
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
    fn renders_a_checkbox_prefix_reflecting_the_marked_state() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1"), sample_issue("ENG-2")]);
        app.move_selection_down();
        app.toggle_mark(); // marks ENG-2 only

        let text = rendered_text(&app);
        assert!(text.contains("[ ] ENG-1"));
        assert!(text.contains("[x] ENG-2"));
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
    fn status_banner_height_stays_at_the_floor_for_a_short_message() {
        assert_eq!(
            status_banner_height("started", 60),
            STATUS_BANNER_MIN_HEIGHT
        );
    }

    #[test]
    fn status_banner_height_grows_with_message_length() {
        let long = "x".repeat(500);
        let height = status_banner_height(&long, 60);

        assert!(height > STATUS_BANNER_MIN_HEIGHT);
        assert!(height <= STATUS_BANNER_MAX_HEIGHT);
    }

    #[test]
    fn status_banner_height_is_clamped_to_a_maximum() {
        let huge = "x".repeat(10_000);
        assert_eq!(status_banner_height(&huge, 60), STATUS_BANNER_MAX_HEIGHT);
    }

    #[test]
    fn status_banner_height_treats_a_zero_width_as_the_floor() {
        assert_eq!(
            status_banner_height("anything", 0),
            STATUS_BANNER_MIN_HEIGHT
        );
    }

    #[test]
    fn status_banner_height_does_not_wrap_around_for_a_pathologically_long_message() {
        // A single underlying herdr/Linear error string isn't length-capped by
        // `main.rs::MAX_STATUS_DETAILS` (that bounds segment *count*, not each segment's
        // length) — a message past `u16::MAX` chars must still saturate to the maximum banner
        // height, not wrap `as u16` around to a small number and under-estimate.
        let pathological = "x".repeat(u16::MAX as usize + 1);
        assert_eq!(
            status_banner_height(&pathological, 1),
            STATUS_BANNER_MAX_HEIGHT
        );
    }

    #[test]
    fn renders_a_long_multi_issue_failure_banner_without_clipping_the_tail() {
        // Regression guard for the pre-TF-590-fix banner: a fixed 3-row area at width 60 (~180
        // usable chars) would have clipped a message this long well before its last segment.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        let details: Vec<String> = (0..8)
            .map(|i| {
                format!(
                    "ENG-{i}: failed to start agent tab: some fairly long underlying herdr error"
                )
            })
            .collect();
        app.set_status(Status::Error(format!(
            "2/8 started, {}",
            details.join("; ")
        )));

        let text = rendered_text_with_size(&app, 60, 20);

        assert!(
            text.contains("ENG-7"),
            "expected the banner's last detail segment to be visible, got: {text}"
        );
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

    // TF-580: type-to-filter rendering.

    #[test]
    fn shows_the_filter_query_and_a_cursor_marker_in_the_list_title_while_editing() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "eng".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let text = rendered_text(&app);
        assert!(text.contains("filter: eng"));
        // The cursor marker (▏) is present while actively editing.
        assert!(text.contains('▏'));
    }

    #[test]
    fn shows_the_confirmed_filter_query_without_a_cursor_marker() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "eng".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        let text = rendered_text(&app);
        assert!(text.contains("filter: eng"));
        assert!(!text.contains('▏'));
    }

    #[test]
    fn only_matching_issues_are_rendered_in_the_list_while_filtering() {
        let mut app = app_in_my_issues_view();
        let mut issues = vec![sample_issue("ENG-1"), sample_issue("ENG-2")];
        issues[0].title = "Fix login bug".to_string();
        issues[1].title = "Add dark mode".to_string();
        app.set_issues(issues);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "login".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let text = rendered_text(&app);
        assert!(text.contains("Fix login bug"));
        assert!(!text.contains("Add dark mode"));
    }

    #[test]
    fn shows_a_no_matches_message_when_the_filter_matches_nothing() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "nonexistent".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }

        let text = rendered_text(&app);
        assert!(text.contains("No issues match"));
        assert!(!text.contains("Title for ENG-1"));
    }
}
