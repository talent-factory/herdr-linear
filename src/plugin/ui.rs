//! Rendering for the plugin TUI: a view-selection menu, a loading message, an error
//! message with a retry hint, or a two-pane issue list + detail view.

use crate::plugin::app::{
    matching_issue_indices, ActivePreset, App, HelpOverlayState, HelpTab, Screen, Status, ViewKind,
    ViewState, MENU_OPTIONS,
};
use crate::Issue;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthChar;

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen() {
        Screen::Menu { selected } => draw_menu(frame, *selected),
        Screen::View(kind, view_state) => {
            draw_view(frame, *kind, view_state, app.status(), app.active_preset())
        }
    }
    if let Some(overlay) = app.help_overlay() {
        draw_help_overlay(frame, overlay);
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

fn draw_view(
    frame: &mut Frame,
    kind: ViewKind,
    view_state: &ViewState,
    status: Option<&Status>,
    active_preset: Option<&ActivePreset>,
) {
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
            detail_scroll,
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
            let list_title = {
                // TF-647: the active named filter preset's name, folded into the label
                // right next to the view name — the same visibility the `/`-filter query
                // text already gets below, just for the *other* way a fetch's query can
                // differ from the plain (unnamed, never shown) `default_query`.
                let label = match active_preset {
                    Some(preset) => format!("{} [{}]", kind.label(), preset.name),
                    None => kind.label().to_string(),
                };
                let base = match (filter.editing, filter.query.is_empty()) {
                    (true, _) => format!("{label} — filter: {}▏", filter.query),
                    (false, true) => label,
                    (false, false) => format!("{label} — filter: {}", filter.query),
                };
                // TF-617 review fix: `matching_issue_indices` silently falls back to a
                // free-text search for a recognized-but-malformed `key:value` term (e.g.
                // `priority:notanumber`) — `ParsedQuery::rejected` exists precisely to
                // surface that, but had no caller before this. Re-parsing `filter.query`
                // here (cheap — same pure function `matching_issue_indices` itself just
                // ran, on a short string) lets a typo be visible in the title instead of
                // just producing a quietly-wrong match list.
                let rejected = if filter.query.is_empty() {
                    Vec::new()
                } else {
                    crate::plugin::query::parse_query(&filter.query).rejected
                };
                if rejected.is_empty() {
                    base
                } else {
                    format!("{base} (⚠ not recognized: {})", rejected.join(", "))
                }
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
                let lines = build_detail_lines(issue, detail_area.width as usize);

                let sections = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(1)])
                    .split(detail_area);

                // `trim: false` preserves each line's leading whitespace, which
                // is exactly what Markdown uses to convey nested lists and
                // indented code — `trim: true` would strip it and flatten that
                // structure away. The header lines above have no leading
                // whitespace to begin with, so this is harmless for them.
                // `.scroll` applies `App::detail_scroll` — moved by the `j`/`k`
                // keybindings in `handle_key` — so a description too long for
                // `sections[0]` is reachable instead of being silently clipped
                // past the bottom.
                let body = Paragraph::new(Text::from(lines))
                    .wrap(Wrap { trim: false })
                    .scroll((*detail_scroll, 0));
                frame.render_widget(body, sections[0]);

                let footer = Paragraph::new(format!("URL: {}", issue.url));
                frame.render_widget(footer, sections[1]);
            }
        }
    }
}

/// Builds the Detail pane's full line list for `issue` — header fields
/// (identifier, title, Status/Assignee/Project) followed by its Markdown
/// description, rewrapped for `width` columns (see
/// [`harden_list_item_wrapping`]). Shared by `draw_view`'s real render (called
/// with the pane's actual, dynamic width) and [`detail_line_count`] (called with
/// [`DETAIL_CONSERVATIVE_WRAP_WIDTH`] instead), so the two can never drift out
/// of sync with each other — see `detail_line_count`'s own doc for why a
/// narrower-than-real assumed width there is the safe direction.
fn build_detail_lines(issue: &Issue, width: usize) -> Vec<Line<'static>> {
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
        Line::from(issue.identifier.clone()),
        Line::from(""),
        Line::from(issue.title.clone()),
        Line::from(""),
        Line::from(format!("Status: {}", issue.state.name)),
        Line::from(format!("Assignee: {assignee}")),
        Line::from(format!("Project: {project}")),
        Line::from(""),
    ];

    let description = issue.description.as_deref().unwrap_or_default();
    let options = tui_markdown::Options::new(MarkdownStyleSheet);
    let markdown_lines = tui_markdown::from_str_with_options(description, &options).lines;
    lines.extend(harden_list_item_wrapping(&markdown_lines, width));
    lines
}

/// The *rendered* row count of the Detail pane's content for `issue` — the number of
/// terminal rows [`build_detail_lines`]'s output will actually occupy once `draw_view`
/// wraps it with `Wrap { trim: false }`, not just the number of logical lines. Lets
/// `App::detail_scroll_down` clamp the stored scroll offset against what will really be
/// on screen, mirroring [`content_line_count`]'s identical role for the help overlay —
/// see that function's doc for the full "narrower-than-real is the safe over-counting
/// direction" rationale, which applies here unchanged. `App` stays deliberately unaware
/// of terminal size, so this uses [`DETAIL_CONSERVATIVE_WRAP_WIDTH`] rather than the
/// Detail pane's real, dynamic width.
pub(crate) fn detail_line_count(issue: &Issue) -> usize {
    build_detail_lines(issue, DETAIL_CONSERVATIVE_WRAP_WIDTH)
        .iter()
        .map(|line| word_wrapped_row_count(&line_plain_text(line), DETAIL_CONSERVATIVE_WRAP_WIDTH))
        .sum()
}

/// Assumed content width (columns) used only to keep [`detail_line_count`]'s scroll-clamp
/// ceiling from running out ahead of the real, wrapped render — see that function's doc
/// for why a narrower-than-real assumption is the safe direction. The Detail pane is the
/// right half of a 50/50 horizontal split (`draw_view`'s `chunks`), minus 2 columns for
/// its bordered `Block`; even an unusually narrow 44-column terminal still leaves
/// `44 / 2 - 2 = 20` columns of real content width. Review fix (PR #44): the first cut of
/// this constant was set to exactly `20` — equal to, not below, that floor, so it carried
/// *zero* actual margin despite its own doc claiming one; any terminal narrower than 44
/// columns would silently violate the "narrower-than-real is safe" invariant
/// `detail_line_count` depends on. `18` restores a genuine 2-column buffer below the
/// 44-column floor (down to a 40-column terminal), mirroring the sibling
/// `CONSERVATIVE_WRAP_WIDTH`'s own real 2-column margin (`30` real vs. `28` assumed) —
/// see the `detail_line_count_estimate_stays_at_or_below_the_real_wrapped_row_count_at_a_
/// narrow_width` test below, which pins the margin down directly rather than relying on
/// this comment's arithmetic staying honest on its own.
const DETAIL_CONSERVATIVE_WRAP_WIDTH: usize = 18;

/// The plain text of `line`, ignoring styling — its spans' `content` concatenated in
/// order. [`word_wrapped_row_count`] only needs the text, not [`build_detail_lines`]'s
/// per-span styling (bold headings, code, etc. from [`MarkdownStyleSheet`]).
fn line_plain_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Bullet substituted for `tui-markdown`'s literal `-` unordered-list marker
/// (see [`harden_list_item_wrapping`]).
const LIST_BULLET: char = '•';

/// Minimum body-text budget (`width - marker_width`, in display columns)
/// [`rewrap_list_item`] requires before hanging-indent-wrapping a list item
/// itself, rather than falling back to a single un-rewrapped row. Below
/// this, hard-breaking would put only a couple of characters per row —
/// technically correct but unreadable, and worse than the ambiguity the
/// wrap exists to fix (see [`rewrap_list_item`]'s fallback comment). Chosen
/// as roughly "a short word still fits" (`"item"` is 4 columns, `"marker"`
/// is 6) rather than derived from any hard constraint.
const MIN_WRAP_BUDGET: usize = 8;

/// Post-processes `tui_markdown::from_str_with_options`'s output (TF-613, a
/// follow-up to TF-583) so a list item's marker can never be mistaken for a
/// wrapped continuation line, and vice versa.
///
/// `tui-markdown` renders an unordered marker as a literal `- ` span (see
/// its `renderer::list` module — not part of the crate's public API, so
/// this necessarily depends on the *shape* of its output rather than a
/// documented contract) and otherwise leaves line-wrapping entirely to
/// `ratatui::widgets::Paragraph`'s `Wrap { trim: false }`, which has no
/// concept of list structure: it hard-wraps at the render width wherever
/// the next word would overflow, with no hanging indent for continuation
/// lines. If a wrapped continuation happens to start with `--` (e.g. inline
/// code like `cargo test --features plugin -- --ignored live_api` wrapping
/// right before `--ignored`), it reads exactly like a new top-level bullet
/// — reproduced live via TF-612's own rendered description.
/// `tui_markdown::StyleSheet`'s ~20 hooks (heading/code/link/blockquote/
/// metadata/footnote/definition-list/table/image/html/math/alert, at the
/// pinned `tui-markdown` version) cover *what style to render with*, not
/// *what glyph a marker uses* or *how a line wraps* — neither is
/// configurable through the trait, so this fix has to happen here,
/// downstream of the crate, as post-processing of its output rather than
/// through the extension point [`MarkdownStyleSheet`] itself uses for its
/// heading/code-fence overrides.
///
/// For each list-item line found (see [`rewrap_list_item`]): the literal
/// `- ` marker is swapped for [`LIST_BULLET`] (same display width, so
/// nothing else in the line shifts), and the line is pre-wrapped to `width`
/// columns before `Paragraph` ever sees it, so every continuation row
/// already carries a hanging indent matching the marker's width. Ordered
/// (`1. `) markers keep their digits — nothing in them collides with body
/// text the way `-`/`--` do — but still get the hanging-indent treatment,
/// since an un-indented wrapped continuation is still visually confusable
/// with the start of the next item.
fn harden_list_item_wrapping(lines: &[Line<'_>], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    lines
        .iter()
        .flat_map(|line| rewrap_list_item(line, width).unwrap_or_else(|| vec![to_owned_line(line)]))
        .collect()
}

/// If `line` is a list-item line as `tui-markdown` renders one — leading
/// spaces (a multiple of 4 for any list depth `tui-markdown` itself would
/// produce; each nesting level indents its marker span by 4 more) followed
/// by a first span holding *only* a marker, either `- ` (unordered) or
/// `<digits>. ` (ordered), with nothing else appended — returns it
/// rewrapped to `width` columns with a hanging indent (see
/// [`wrap_list_item_body`]), swapping a leading `-` for [`LIST_BULLET`].
/// Returns `None` for any other line, which the caller passes through
/// unchanged — including a line that merely *starts with* the marker
/// pattern but carries more text in the same span (a fenced code-block
/// line, an escaped `\- ` paragraph): `tui-markdown` never merges body text
/// into the marker span, so that shape is never a real list item. A
/// task-list checkbox (`[ ] `/`[x] `) is recognized whether `tui-markdown`
/// places it in the same span as the marker (unordered) or as the
/// following span (ordered) — see `renderer::list::task_list_marker`.
fn rewrap_list_item(line: &Line<'_>, width: usize) -> Option<Vec<Line<'static>>> {
    let first = line.spans.first()?;
    let content = first.content.as_ref();
    let indent_len = content.len() - content.trim_start_matches(' ').len();
    if indent_len % 4 != 0 {
        return None;
    }
    let after_indent = &content[indent_len..];

    let is_unordered = after_indent.starts_with("- ");
    let digits_end = after_indent
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(0);
    let is_ordered = digits_end > 0 && after_indent[digits_end..].starts_with(". ");
    if !is_unordered && !is_ordered {
        return None;
    }

    // `tui-markdown` always renders a list marker as its own span holding
    // nothing but the marker itself (plus, for an unordered item, an inline
    // task checkbox) — body text always lives in later spans. A line whose
    // first span merely *starts with* the marker pattern but carries more
    // text after it — a fenced code-block line (`- old line` inside a
    // ```diff block), an escaped `\- ` paragraph — is therefore not a real
    // list marker; leave it untouched rather than mangling its content.
    let marker_end = if is_unordered { 2 } else { digits_end + 2 };
    let rest = &after_indent[marker_end..]; // empty, or a task checkbox like "[ ] "
    if !(rest.is_empty() || rest == "[ ] " || rest == "[x] ") {
        return None;
    }

    let mut spans: Vec<Span<'static>> = line.spans.iter().map(to_owned_span).collect();

    if is_unordered {
        let indent = &content[..indent_len];
        spans[0] = Span::styled(format!("{indent}{LIST_BULLET} {rest}"), spans[0].style);
    }

    let marker_span_count = if is_ordered
        && spans
            .get(1)
            .is_some_and(|span| matches!(span.content.as_ref(), "[ ] " | "[x] "))
    {
        2
    } else {
        1
    };
    let marker_width: usize = spans[..marker_span_count].iter().map(Span::width).sum();

    // Below a minimum body-text budget — deep nesting in a narrow pane —
    // `wrap_list_item_body`'s hanging-indent wrap degenerates into a
    // spindly, near-unreadable character-per-row hard-break, which is worse
    // than the ambiguity this whole function exists to fix. The `•` marker
    // substitution doesn't depend on the hanging indent for its
    // disambiguation, though: a bullet is never confused with a plain `-`
    // continuation regardless of indentation. So below that floor, fall
    // back to just the marker swap on a single un-rewrapped row and leave
    // wrapping the body to `Paragraph`'s own `Wrap`.
    if width.saturating_sub(marker_width) < MIN_WRAP_BUDGET {
        return Some(vec![Line {
            spans,
            style: line.style,
            alignment: line.alignment,
        }]);
    }

    let body_spans = spans.split_off(marker_span_count);

    let mut rows = wrap_list_item_body(spans, body_spans, width, marker_width);
    // Every rebuilt row is a continuation of the same source line, so it
    // should carry that line's own style/alignment — the same thing
    // `to_owned_line` (this function's fallback for non-list lines, right
    // below) already preserves for an unmodified line.
    for row in &mut rows {
        row.style = line.style;
        row.alignment = line.alignment;
    }
    Some(rows)
}

/// Clones `span`'s content into an owned `String` so it outlives the
/// borrowed `tui_markdown::from_str_with_options` output it came from —
/// needed because [`harden_list_item_wrapping`] returns `Line<'static>`.
fn to_owned_span(span: &Span<'_>) -> Span<'static> {
    Span::styled(span.content.to_string(), span.style)
}

/// [`to_owned_span`] applied to every span of `line`, preserving the line's
/// own `style`/`alignment` — the fallback [`harden_list_item_wrapping`]
/// uses for any line [`rewrap_list_item`] didn't recognize as a list item.
fn to_owned_line(line: &Line<'_>) -> Line<'static> {
    Line {
        spans: line.spans.iter().map(to_owned_span).collect(),
        style: line.style,
        alignment: line.alignment,
    }
}

/// One space-delimited word from a list item's body text, tokenized
/// character-by-character (each retaining its source [`Style`]) so
/// [`wrap_list_item_body`] can rebuild it as one or more styled `Span`s — a
/// single word can straddle a Markdown style boundary, e.g. the word in
/// `**bold *emphasis***` spans both a bold-only and a bold+italic run.
struct Word {
    chars: Vec<(char, Style)>,
    /// Style and run-length of the whitespace that followed this word in the
    /// source spans (`None` when nothing did — ordinarily only the last
    /// word, unless the body text itself has trailing whitespace). Reused
    /// for a same-row
    /// separator so a run of spaces — e.g. deliberate column-alignment
    /// inside one long inline-code span — survives the rewrap intact
    /// (matching `Paragraph`'s own `Wrap { trim: false }`, which this
    /// replaces for list items and does not collapse whitespace either)
    /// instead of being collapsed to a single default-styled space.
    trailing_space: Option<(Style, usize)>,
}

impl Word {
    fn char_len(&self) -> usize {
        self.chars.len()
    }

    /// Display-column width of the whole word — sum of each character's
    /// [`unicode_width`] column count, NOT [`Word::char_len`]. Most CJK
    /// ideographs and many emoji occupy 2 terminal columns despite being a
    /// single `char`/Unicode scalar value; wrap-budget decisions must use
    /// this, matching how `marker_width` is measured (`Span::width`), or a
    /// row containing such characters silently overflows the target width.
    fn width(&self) -> usize {
        self.chars
            .iter()
            .map(|&(c, _)| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum()
    }

    /// Greedily selects the leading characters (starting at char index
    /// `start`) that fit within `budget` display columns — always at least
    /// one character, to guarantee forward progress even when a single
    /// character's own width exceeds `budget` (a pathologically narrow
    /// pane). Returns `(chars_taken, display_width_taken)`; the former
    /// indexes [`Word::spans`], the latter becomes the caller's `col`.
    fn take_within(&self, start: usize, budget: usize) -> (usize, usize) {
        let mut width = 0usize;
        let mut count = 0usize;
        for &(c, _) in &self.chars[start..] {
            let char_width = UnicodeWidthChar::width(c).unwrap_or(0);
            if count > 0 && width + char_width > budget {
                break;
            }
            width += char_width;
            count += 1;
            if width >= budget {
                break;
            }
        }
        (count, width)
    }

    /// Rebuilds `self.chars[range]` as the minimal run of styled `Span`s,
    /// merging consecutive same-style characters.
    fn spans(&self, range: std::ops::Range<usize>) -> Vec<Span<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for &(c, style) in &self.chars[range] {
            match spans.last_mut() {
                Some(last) if last.style == style => last.content.to_mut().push(c),
                _ => spans.push(Span::styled(c.to_string(), style)),
            }
        }
        spans
    }
}

/// Splits `spans` into [`Word`]s on space characters, preserving the exact
/// run-length of each separator (see [`Word::trailing_space`]) and
/// discarding leading/trailing whitespace.
fn styled_words(spans: &[Span<'static>]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut current: Vec<(char, Style)> = Vec::new();

    for span in spans {
        for c in span.content.chars() {
            if c == ' ' {
                if !current.is_empty() {
                    words.push(Word {
                        chars: std::mem::take(&mut current),
                        trailing_space: Some((span.style, 1)),
                    });
                } else if let Some(last) = words.last_mut() {
                    match &mut last.trailing_space {
                        Some((_, len)) => *len += 1,
                        None => last.trailing_space = Some((span.style, 1)),
                    }
                }
            } else {
                current.push((c, span.style));
            }
        }
    }
    if !current.is_empty() {
        words.push(Word {
            chars: current,
            trailing_space: None,
        });
    }
    words
}

/// Word-wraps `body_spans` to `width` columns, prefixed by `marker_spans`
/// (kept verbatim, never re-flowed) on the first row and a `marker_width`-
/// wide run of spaces — the hanging indent — on every row after. Mirrors
/// the greedy word-wrap [`word_wrapped_row_count`] already models for
/// ratatui's `Wrap { trim: false }` (pack space-separated words; start a
/// new row when the next word wouldn't fit; hard-break a lone word wider
/// than the available row budget), but builds the actual wrapped `Line`s
/// instead of just counting them.
fn wrap_list_item_body(
    marker_spans: Vec<Span<'static>>,
    body_spans: Vec<Span<'static>>,
    width: usize,
    marker_width: usize,
) -> Vec<Line<'static>> {
    let budget = width.saturating_sub(marker_width).max(1);
    let words = styled_words(&body_spans);

    let mut rows: Vec<Vec<Span<'static>>> = vec![marker_spans];
    let mut col = 0usize; // display columns used so far on the current row
    let new_row = || vec![Span::raw(" ".repeat(marker_width))];

    for (i, word) in words.iter().enumerate() {
        // `word_width` (display columns, via `Word::width`) drives every fit
        // decision below, matching the unit `budget`/`marker_width`/`col`
        // are already measured in. `word.char_len()`/`word.spans(range)`
        // stay char-index based (that's what indexes `self.chars`), so a
        // wide-character word's *count* of taken characters and its
        // *display width* are tracked separately throughout.
        let word_width = word.width();

        if word_width > budget {
            if col > 0 {
                rows.push(new_row());
            }
            let char_len = word.char_len();
            let mut start = 0;
            loop {
                let (take, taken_width) = word.take_within(start, budget);
                rows.last_mut()
                    .expect("at least the marker row exists")
                    .extend(word.spans(start..start + take));
                start += take;
                if start >= char_len {
                    col = taken_width;
                    break;
                }
                rows.push(new_row());
            }
        } else {
            let needed = if col == 0 {
                word_width
            } else {
                let (_, separator_len) =
                    words[i - 1].trailing_space.unwrap_or((Style::default(), 1));
                col + separator_len + word_width
            };
            if needed <= budget {
                if col > 0 {
                    let (separator_style, separator_len) =
                        words[i - 1].trailing_space.unwrap_or((Style::default(), 1));
                    rows.last_mut()
                        .expect("at least the marker row exists")
                        .push(Span::styled(" ".repeat(separator_len), separator_style));
                }
                rows.last_mut()
                    .expect("at least the marker row exists")
                    .extend(word.spans(0..word.char_len()));
                col = needed;
            } else {
                rows.push(new_row());
                rows.last_mut()
                    .expect("at least the marker row exists")
                    .extend(word.spans(0..word.char_len()));
                col = word_width;
            }
        }
    }

    let lines: Vec<Line<'static>> = rows.into_iter().map(Line::from).collect();
    debug_assert!(
        lines.iter().all(|row| row.width() <= width),
        "wrap_list_item_body produced a row wider than its {width}-column budget: {lines:?}"
    );
    lines
}

/// The About tab's content (TF-585): plugin name, version, description, repo, license —
/// all resolved at compile time from `Cargo.toml` via `CARGO_PKG_*` env vars, so there's
/// nothing to keep in sync by hand when either changes.
///
/// Follow-up review fix (TF-585): cached in a `OnceLock` — every call previously
/// rebuilt this `Vec` from scratch, and `content_line_count` (the scroll-clamp's
/// production caller, in `app.rs`'s `j`/`↓` handler) calls the active tab's content
/// function on *every* scroll keypress purely to measure its length, discarding the
/// content itself. Safe to cache unconditionally: every source here (`env!` macros) is
/// resolved at compile time, so the result can never change within a running process —
/// unlike [`settings_lines`], which reads the real, mutable environment on every call and
/// deliberately stays uncached.
fn about_lines() -> Vec<String> {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            vec![
                format!("herdr-linear v{}", env!("CARGO_PKG_VERSION")),
                String::new(),
                env!("CARGO_PKG_DESCRIPTION").to_string(),
                String::new(),
                format!("Repository: {}", env!("CARGO_PKG_REPOSITORY")),
                format!("License: {}", env!("CARGO_PKG_LICENSE")),
            ]
        })
        .clone()
}

/// The Keybindings tab's content (TF-585): every entry in `keybindings::KEYBINDINGS`
/// (the single source of truth — see that module's doc comment), grouped under a
/// heading each time `context` changes. Relies on `KEYBINDINGS` grouping same-context
/// entries contiguously (an invariant that table's own tests guard) rather than
/// re-sorting, so the table's declared order (Menu, View, Filtering, Error screen,
/// Global) is what's shown, not an alphabetized one.
///
/// Cached in a `OnceLock` for the same reason as [`about_lines`]: `KEYBINDINGS` is a
/// `static` table that never changes within a running process, so recomputing this on
/// every scroll keypress is pure waste.
fn keybindings_lines() -> Vec<String> {
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut lines = Vec::new();
            let mut last_context: Option<crate::plugin::keybindings::BindingContext> = None;

            for binding in crate::plugin::keybindings::KEYBINDINGS {
                if last_context != Some(binding.context) {
                    if last_context.is_some() {
                        lines.push(String::new());
                    }
                    lines.push(format!("{}:", binding.context.label()));
                    last_context = Some(binding.context);
                }
                lines.push(format!("  {:<10} {}", binding.keys, binding.action));
            }

            lines
        })
        .clone()
}

/// Everything between `heading` (matched verbatim, must be a full `## ...` heading line
/// present in `text`) and the next `## ` heading (or end of `text`), with leading/
/// trailing blank lines trimmed off (interior blank lines between entries are kept).
/// `None` if `heading` isn't found, or if the section is empty after trimming — both
/// read as "nothing here" to callers (see `whats_new_lines_from`, which falls back to
/// the next section in either case).
fn extract_section_after(text: &str, heading: &str) -> Option<Vec<String>> {
    let start = text.find(heading)?;
    let after_heading = &text[start + heading.len()..];
    let end = after_heading.find("\n## ").unwrap_or(after_heading.len());
    let body: Vec<&str> = after_heading[..end].lines().collect();

    let first_non_blank = body.iter().position(|line| !line.trim().is_empty())?;
    let last_non_blank = body.iter().rposition(|line| !line.trim().is_empty())?;
    Some(
        body[first_non_blank..=last_non_blank]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )
}

/// The What's New tab's content (TF-585): the current version as a heading, followed by
/// `CHANGELOG.md`'s `[Unreleased]` entries — embedded at compile time via `include_str!`
/// so the plugin binary never depends on `CHANGELOG.md` being present at runtime (it
/// isn't; nothing ships the source repo alongside the built binary).
fn whats_new_lines() -> Vec<String> {
    // Cached in a `OnceLock` for the same reason as `about_lines`/`keybindings_lines`:
    // `include_str!` embeds `CHANGELOG.md` at compile time, so the content can never
    // change within a running process, and this is otherwise recomputed (including a
    // full re-parse of the embedded changelog) on every scroll keypress.
    static CACHE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| whats_new_lines_from(include_str!("../../CHANGELOG.md")))
        .clone()
}

/// Pure half of [`whats_new_lines`], taking the changelog content as a parameter so it's
/// testable against fixture strings without depending on the real `CHANGELOG.md` —
/// mirrors `config.rs`'s `resolve_api_key`/`load` split (pure core + thin
/// compile-time-data wrapper).
fn whats_new_lines_from(changelog: &str) -> Vec<String> {
    const UNRELEASED_HEADING: &str = "## [Unreleased]";
    let mut lines = vec![
        format!("v{} (unreleased)", env!("CARGO_PKG_VERSION")),
        String::new(),
    ];

    let section = extract_section_after(changelog, UNRELEASED_HEADING).or_else(|| {
        // `[Unreleased]` is missing or empty — fall back to the next `## [` heading
        // after it (the newest real release). Search from just past `[Unreleased]`'s
        // own heading line so this can't just re-find the same empty section; if
        // `[Unreleased]` isn't present at all, search the whole file.
        let search_from = changelog
            .find(UNRELEASED_HEADING)
            .and_then(|i| changelog[i..].find('\n').map(|nl| i + nl + 1))
            .unwrap_or(0);
        let rest = &changelog[search_from..];
        let heading_line_start = rest.find("## [")?;
        let heading_line_end = rest[heading_line_start..]
            .find('\n')
            .map(|nl| heading_line_start + nl)
            .unwrap_or(rest.len());
        extract_section_after(
            &rest[heading_line_start..],
            &rest[heading_line_start..heading_line_end],
        )
    });

    match section {
        Some(entries) => lines.extend(entries),
        None => lines.push("Couldn't find recent changes in CHANGELOG.md.".to_string()),
    }

    truncate_with_notice(&mut lines, WHATS_NEW_MAX_LINES);
    lines
}

/// Hard cap on how many lines [`whats_new_lines_from`] shows before truncating with a
/// pointer to the full changelog (follow-up review fix, TF-585 — found during code
/// review: `CHANGELOG.md`'s real `[Unreleased]` section already runs past 100 lines,
/// spanning every generic library-scaffolding entry since the project's only release,
/// not just recent, plugin-facing work, which defeats the point of a "what's new"
/// summary meant to be read in a small overlay panel). Sized to comfortably fit a normal
/// terminal without excessive scrolling for a typical release cycle's worth of entries;
/// deliberately a *display* limit in `ui.rs`; the underlying `CHANGELOG.md` is untouched
/// and remains the full, authoritative history.
const WHATS_NEW_MAX_LINES: usize = 30;

/// Truncates `lines` to at most `max` entries, replacing anything past that with a
/// single "… N more lines" notice pointing at `CHANGELOG.md` — so an oversized section
/// degrades *visibly* (the reader knows there's more, and where to find it) rather than
/// silently. No-op if `lines` is already within `max`. `max` must be at least `1`.
fn truncate_with_notice(lines: &mut Vec<String>, max: usize) {
    debug_assert!(max >= 1, "truncate_with_notice requires max >= 1");
    if lines.len() <= max {
        return;
    }
    let kept = max - 1; // one of `max` slots is reserved for the notice itself
    let hidden = lines.len() - kept;
    lines.truncate(kept);
    lines.push(format!(
        "… {hidden} more line{} — see CHANGELOG.md for the full history.",
        if hidden == 1 { "" } else { "s" }
    ));
}

/// The Settings tab's content (TF-585): the plugin's currently-resolved `config.toml`
/// values. Reads the real environment once, via the same `HERDR_PLUGIN_CONFIG_DIR`/
/// `LINEAR_API_KEY` lookup `config::load()` uses, then hands off to
/// `config::resolved_summary` for the actual resolution logic — this function owns no
/// config-reading of its own, only formatting the result.
fn settings_lines() -> Vec<String> {
    let config_dir = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(std::path::PathBuf::from);

    // `var_os` (not `var().ok()`) so a `LINEAR_API_KEY` that's set but not valid UTF-8
    // can be told apart from one that's simply unset (follow-up review fix, TF-585): the
    // config-resolution path genuinely can't use a non-Unicode value either way (see
    // `config::resolve_api_key`, which hits the identical `var().ok()` collapse), so
    // `resolved_summary` still correctly reports `api_key_set: false` for it — but a
    // diagnostic tab whose entire purpose is explaining *why* a key isn't resolving
    // should say so, not silently show the same "✗ Not set" as a key that was never
    // exported at all.
    let env_api_key_os = std::env::var_os("LINEAR_API_KEY");
    let env_key_present_but_not_utf8 = env_api_key_os
        .as_deref()
        .is_some_and(|v| !v.is_empty() && v.to_str().is_none());
    let env_api_key = env_api_key_os.as_deref().and_then(|v| v.to_str());

    let summary = crate::plugin::config::resolved_summary(config_dir.as_deref(), env_api_key);
    settings_lines_from(&summary, env_key_present_but_not_utf8)
}

/// Pure half of [`settings_lines`], taking an already-resolved summary so it's testable
/// without touching the real environment. `env_key_present_but_not_utf8` is the one piece
/// of diagnostic state `ResolvedConfigSummary` can't carry on its own (it only knows
/// `api_key_set: bool`, which is correctly `false` in this case too, since a non-Unicode
/// env var is just as unusable for authentication as a missing one) — see
/// [`settings_lines`]'s doc comment for why the Settings tab still needs to tell the two
/// "not set" cases apart in its own display text.
fn settings_lines_from(
    summary: &crate::plugin::config::ResolvedConfigSummary,
    env_key_present_but_not_utf8: bool,
) -> Vec<String> {
    use crate::plugin::config::ConfigFileStatus;

    let mut lines = Vec::new();
    match &summary.status {
        ConfigFileStatus::NotFound => {
            lines.push("Config: no file found, using defaults.".to_string())
        }
        ConfigFileStatus::Found => lines.push("Config: found".to_string()),
        ConfigFileStatus::Invalid(message) => lines.push(format!(
            "Config: {} exists but is invalid — {message}",
            summary.path
        )),
    }
    lines.push(format!("Location: {}", summary.path));
    lines.push(String::new());

    let api_key_display = if summary.api_key_set {
        "✓ Set"
    } else if env_key_present_but_not_utf8 {
        "✗ Not set (LINEAR_API_KEY is set but isn't valid UTF-8)"
    } else {
        "✗ Not set"
    };
    lines.push(format!("api_key          = {api_key_display}"));

    let agent_command_display = summary.agent_command.as_deref().unwrap_or("(default)");
    lines.push(format!("agent_command    = {agent_command_display}"));

    let editor_display = summary
        .editor
        .as_deref()
        .unwrap_or("(default: nvim if on PATH)");
    lines.push(format!("editor           = {editor_display}"));

    let team_id_display = summary.team_id.as_deref().unwrap_or("Not set");
    lines.push(format!("team_id          = {team_id_display}"));

    let default_query_display = summary.default_query.as_deref().unwrap_or("Not set");
    lines.push(format!("default_query    = {default_query_display}"));

    if summary.project_overrides.is_empty() {
        lines.push("project_overrides: (none)".to_string());
    } else {
        lines.push("project_overrides:".to_string());
        for (repo, project_id) in &summary.project_overrides {
            lines.push(format!("  {repo:<15} = {project_id}"));
        }
    }

    // TF-647: named filter presets, in declaration order — mirrors project_overrides'
    // list-or-"(none)" shape just above.
    if summary.filter_presets.is_empty() {
        lines.push("filter_presets: (none)".to_string());
    } else {
        lines.push("filter_presets:".to_string());
        for preset in &summary.filter_presets {
            lines.push(format!("  {:<15} = {}", preset.name, preset.query));
        }
    }

    lines
}

/// The *rendered* row count of `tab`'s content — the number of terminal rows it will
/// actually occupy once `draw_help_overlay` wraps it with `Wrap { trim: false }`, not
/// just the number of logical `\n`-separated entries. Lets `App::help_overlay_scroll_down`
/// (final-review fix, TF-585) clamp the stored scroll offset against what will really be
/// on screen, since that's otherwise only known here in `ui.rs`.
///
/// Follow-up review fix (TF-585): this used to return the raw `Vec<String>` length —
/// correct only if every entry is short enough to never wrap. `ratatui::Paragraph::scroll`
/// offsets by *rendered* rows, so any tab with a line wider than the popup (plausible for
/// `whats_new_lines()`, which pulls raw prose from `CHANGELOG.md`) would have more visual
/// rows than the old count reported — clamping `j`/`↓` short of the real end and making
/// the tail of that tab's content permanently unreachable. This estimates wrapped rows via
/// [`word_wrapped_row_count`] against [`CONSERVATIVE_WRAP_WIDTH`] instead of the real,
/// dynamic terminal width — `App` is deliberately kept unaware of terminal size (it stays
/// a plain, headlessly-testable key-event state machine), and the assumed width being
/// narrower than any realistic terminal's popup content area means this can only
/// *over*-estimate rows relative to the true, wider render (greedy word-wrap needs the
/// same or more rows at a narrower width, never fewer), which is the safe direction: a
/// few extra scrollable rows land on the same last screen `Paragraph::scroll` already
/// clips gracefully, rather than the real bug this guards against — under-counting, which
/// would make content unreachable.
pub(crate) fn content_line_count(tab: HelpTab) -> usize {
    let lines = match tab {
        HelpTab::WhatsNew => whats_new_lines(),
        HelpTab::Keybindings => keybindings_lines(),
        HelpTab::Settings => settings_lines(),
        HelpTab::About => about_lines(),
    };
    lines
        .iter()
        .map(|line| word_wrapped_row_count(line, CONSERVATIVE_WRAP_WIDTH))
        .sum()
}

/// Assumed content width (columns) used only to keep [`content_line_count`]'s
/// scroll-clamp ceiling from running out ahead of the real, wrapped render — see that
/// function's doc comment for why a narrower-than-real assumption is the safe direction.
/// The popup itself is `centered_rect(80, 90, frame.area())`'s width, minus 2 columns for
/// the bordered `Block`; even an unusually narrow 40-column terminal still leaves
/// `40 * 0.8 - 2 = 30` columns of real content width, so this stays at or below that for
/// a comfortable margin without being so small the estimate balloons needlessly.
const CONSERVATIVE_WRAP_WIDTH: usize = 28;

/// The number of rows a single logical `line` would occupy once greedily word-wrapped to
/// at most `width` columns — closely mirroring how ratatui's `Wrap` widget breaks text
/// (pack space-separated words; when the next word wouldn't fit, start a new row; a lone
/// word wider than `width` on its own is hard-broken across `word_len.div_ceil(width)`
/// rows). An empty line still occupies exactly one (blank) row, matching how a blank line
/// renders. Deliberately approximate — see [`content_line_count`] for why an
/// approximation biased toward *over*-counting rows is the safe choice here, not a bug.
fn word_wrapped_row_count(line: &str, width: usize) -> usize {
    let width = width.max(1);
    if line.is_empty() {
        return 1;
    }

    let mut rows = 1usize;
    let mut col = 0usize; // columns used so far on the current row

    for word in line.split(' ') {
        let word_len = word.chars().count();

        if word_len > width {
            // A single word wider than the whole row: hard-break it across its own
            // rows, then carry on with whatever comes after it on a fresh row.
            if col > 0 {
                rows += 1;
            }
            rows += word_len.div_ceil(width) - 1;
            col = word_len - (word_len.div_ceil(width) - 1) * width;
            continue;
        }

        let needed = if col == 0 {
            word_len
        } else {
            col + 1 + word_len
        };
        if needed <= width {
            col = needed;
        } else {
            rows += 1;
            col = word_len;
        }
    }

    rows
}

/// Renders the help overlay (`?` — TF-585) on top of whatever `draw` already drew for
/// the current screen: `Clear` the area first (ratatui doesn't blank a widget's
/// background on its own — without this, stale content from beneath shows through
/// wherever this frame's text doesn't happen to overwrite it), then the tab bar +
/// scrollable body + footer, matching the herdr-file-viewer reference screenshot this
/// design follows.
fn draw_help_overlay(frame: &mut Frame, overlay: &HelpOverlayState) {
    let area = centered_rect(80, 90, frame.area());
    frame.render_widget(Clear, area);

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // The active tab is marked with a reversed-video style (follow-up review fix,
    // TF-585 — a plain "> " text prefix, as this used before, was too easy to miss at a
    // glance, per user feedback screenshotting the running app). Matches `draw_menu`'s
    // own selection highlight (`Modifier::REVERSED`) so "currently selected" reads the
    // same way everywhere in this UI, not as a one-off convention just for this overlay.
    let mut title_spans: Vec<Span> = vec![Span::raw("Help: ")];
    for (i, &tab) in HelpTab::ALL.iter().enumerate() {
        if i > 0 {
            title_spans.push(Span::raw("   "));
        }
        if tab == overlay.tab {
            title_spans.push(Span::styled(
                tab.title(),
                Style::default().add_modifier(Modifier::REVERSED),
            ));
        } else {
            title_spans.push(Span::raw(tab.title()));
        }
    }

    let content = match overlay.tab {
        HelpTab::WhatsNew => whats_new_lines(),
        HelpTab::Keybindings => keybindings_lines(),
        HelpTab::Settings => settings_lines(),
        HelpTab::About => about_lines(),
    }
    .join("\n");

    let body = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(title_spans))
        .wrap(Wrap { trim: false })
        .scroll((overlay.scroll, 0));
    frame.render_widget(body, outer[0]);

    let footer = Paragraph::new("Tab/←→ switch · 1-4 jump · j/k scroll · Esc/q/? close")
        .style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(footer, outer[1]);
}

/// A `Rect` centered within `area`, `percent_width`/`percent_height` of its size — the
/// standard ratatui popup-centering recipe (two nested percentage-based `Layout` splits,
/// taking the middle cell of each).
fn centered_rect(
    percent_width: u16,
    percent_height: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_height) / 2),
            Constraint::Percentage(percent_height),
            Constraint::Percentage((100 - percent_height) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_width) / 2),
            Constraint::Percentage(percent_width),
            Constraint::Percentage((100 - percent_width) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::app::{handle_key, App};
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{backend::TestBackend, layout::Alignment, Terminal};
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

    /// Like [`rendered_text_with_size`], but keeps row boundaries instead of flattening the
    /// whole buffer into one `String` — needed whenever a test cares which *row* content
    /// lands on (e.g. TF-613's hanging indent), which `.contains()` on a fully flattened
    /// string can't distinguish from content merely appearing later in the same buffer.
    fn rendered_rows_with_size(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    /// Like [`rendered_text_with_size`], but keeps each cell's [`Modifier`] alongside its
    /// symbol instead of discarding it — needed to verify a *visual* highlight (e.g.
    /// `Modifier::REVERSED`), which `rendered_text_with_size`'s plain-`String` output has
    /// no way to represent. Cell order matches the buffer's own row-major layout, same as
    /// `rendered_text_with_size`.
    fn rendered_cells_with_size(app: &App, width: u16, height: u16) -> Vec<(String, Modifier)> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| (cell.symbol().to_string(), cell.modifier))
            .collect()
    }

    /// The start index of the first contiguous run of `symbols` matching `needle`,
    /// character by character — used to locate a known label (e.g. a tab name) within
    /// [`rendered_cells_with_size`]'s output so its cells' styling can be inspected.
    fn find_cell_run(symbols: &[String], needle: &str) -> Option<usize> {
        let needle_chars: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        if needle_chars.is_empty() || symbols.len() < needle_chars.len() {
            return None;
        }
        (0..=symbols.len() - needle_chars.len())
            .find(|&start| symbols[start..start + needle_chars.len()] == needle_chars[..])
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
        // TF-613: unordered list markers render as a distinct bullet, not
        // the literal `-` Markdown syntax.
        assert!(text.contains("• item one"));
        assert!(text.contains("• item two"));
        assert!(!text.contains("- item one"));
        assert!(!text.contains("- item two"));
    }

    #[test]
    fn wrapped_list_item_continuation_is_not_mistaken_for_a_new_bullet() {
        // TF-613 regression: reproduces, deterministically, the exact shape
        // that made TF-612's own rendered description read as two bullets
        // instead of one — a single unordered list item long enough to
        // wrap, with a `--`-prefixed word landing right at the wrap
        // boundary (there, inline code wrapping right before `--ignored`).
        //
        // Total width 100 -> the detail pane's Markdown content is 48
        // columns wide (a 50/50 pane split, minus the 2-column block
        // border), leaving 46 columns of body-text budget after the
        // 2-column "• " marker. `long_a` (25) + " " + `long_b` (20) fills
        // that budget exactly (25 + 1 + 20 = 46), forcing `--ignored-flag`
        // onto its own, hanging-indented continuation row.
        let long_a = "a".repeat(25);
        let long_b = "b".repeat(20);
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-7",
            &format!("- {long_a} {long_b} --ignored-flag"),
        )]);

        let total_width: u16 = 100;
        let rows = rendered_rows_with_size(&app, total_width, 30);
        // The detail pane occupies the right half of the terminal, minus
        // its own left/right border column.
        let detail_start = (total_width / 2 + 1) as usize;
        let detail_end = (total_width - 1) as usize;
        let detail_content = |row: &str| -> String {
            row.chars()
                .skip(detail_start)
                .take(detail_end - detail_start)
                .collect()
        };

        let marker_row = rows
            .iter()
            .map(|row| detail_content(row))
            .position(|content| content.starts_with('•'))
            .expect("the list item's bullet row should be present");
        let continuation = detail_content(&rows[marker_row + 1]);

        assert!(detail_content(&rows[marker_row]).starts_with(&format!("• {long_a}")));
        // The continuation carries the deferred text …
        assert!(continuation.contains("--ignored-flag"));
        // … but under a hanging indent matching the marker's width, not
        // literal dashes at column 0 — the exact ambiguity TF-613 reports.
        assert!(continuation.starts_with("  --ignored-flag"));
        assert!(!continuation.starts_with("--"));
    }

    #[test]
    fn nested_list_items_keep_a_deeper_indent_and_bullet() {
        // TF-613 spot-check: nesting shouldn't be made worse by the marker/
        // indent fix. `tui-markdown` indents each nesting level's marker
        // span by 4 columns; the replaced bullet should follow the same
        // convention.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-8",
            "- Top item\n  - Nested item\n- Second top item",
        )]);

        let text = rendered_text_with_size(&app, 100, 30);
        assert!(text.contains("• Top item"));
        assert!(text.contains("    • Nested item"));
        assert!(text.contains("• Second top item"));
        assert!(!text.contains("- Top item"));
        assert!(!text.contains("- Nested item"));
        assert!(!text.contains("- Second top item"));
    }

    #[test]
    fn ordered_list_item_keeps_its_digits_and_gets_a_hanging_indent_on_wrap() {
        // TF-613: ordered markers keep their digits (no bullet
        // substitution — nothing in them collides with body text the way
        // `-`/`--` do) but still get the hanging-indent treatment on wrap,
        // since an un-indented wrapped continuation is just as visually
        // confusable with the start of the next item.
        let line = Line::from(vec![Span::raw("1. "), Span::raw("aaaaa bbbbb ccccc ddddd")]);
        let width = 12; // marker_width = 3, budget = 9

        let rewrapped = rewrap_list_item(&line, width).expect("a `1. ` line is a list item");

        assert!(rewrapped.len() > 1, "test setup should force a wrap");
        let first: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let second: String = rewrapped[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(first.starts_with("1. aaaaa"));
        assert!(
            second.starts_with("   "),
            "continuation row should carry a 3-column hanging indent matching \"1. \", got {second:?}"
        );
        assert!(!second.starts_with("1."));
    }

    #[test]
    fn unordered_task_checkbox_marker_wraps_with_a_hanging_indent_matching_its_width() {
        // TF-613: `tui-markdown` folds an unordered task-list checkbox into
        // the same span as the `- ` marker itself; the whole thing (bullet
        // + checkbox) is the marker, and the hanging indent must match its
        // full width, not just the bullet's.
        let line = Line::from(vec![
            Span::raw("- [ ] "),
            Span::raw("aaaaa bbbbb ccccc ddddd"),
        ]);
        let width = 14; // marker_width = 6 ("• [ ] "), budget = 8

        let rewrapped = rewrap_list_item(&line, width).expect("a checkbox item is a list item");

        assert!(rewrapped.len() > 1, "test setup should force a wrap");
        let first: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let second: String = rewrapped[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(first.starts_with("• [ ] aaaaa"));
        assert!(
            second.starts_with("      "),
            "continuation row should carry a 6-column hanging indent matching \"• [ ] \", got {second:?}"
        );
    }

    #[test]
    fn ordered_task_checkbox_marker_wraps_with_a_hanging_indent_matching_its_width() {
        // TF-613: for an *ordered* task-list item, `tui-markdown` places
        // the checkbox in its own span right after the digit marker
        // (rather than folding it into the marker span the way it does for
        // unordered items) — `rewrap_list_item` must recognize that second
        // span as still part of the marker, not the wrappable body.
        let line = Line::from(vec![
            Span::raw("1. "),
            Span::raw("[ ] "),
            Span::raw("aaaaa bbbbb ccccc ddddd"),
        ]);
        let width = 17; // marker_width = 7 ("1. [ ] "), budget = 10

        let rewrapped =
            rewrap_list_item(&line, width).expect("an ordered checkbox item is a list item");

        assert!(rewrapped.len() > 1, "test setup should force a wrap");
        let first: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let second: String = rewrapped[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(first.starts_with("1. [ ] aaaaa"));
        assert!(
            second.starts_with("       "),
            "continuation row should carry a 7-column hanging indent matching \"1. [ ] \", got {second:?}"
        );
        assert!(!second.starts_with("1."));
    }

    #[test]
    fn a_single_word_wider_than_the_wrap_budget_is_hard_broken_across_rows() {
        // TF-613: `wrap_list_item_body`'s own hard-break path (distinct
        // from the sibling scroll-estimate function `word_wrapped_row_count`,
        // which has its own hard-break tests below) must still land every
        // character of an over-wide word somewhere, with each continuation
        // row carrying the same hanging indent as an ordinary wrap.
        let long_word = "a".repeat(20);
        let line = Line::from(vec![Span::raw("- "), Span::raw(long_word)]);
        let width = 10; // marker_width = 2, budget = 8

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert_eq!(rewrapped.len(), 3, "ceil(20 / 8) == 3 rows");
        let rows: Vec<String> = rewrapped
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(rows[0], format!("• {}", "a".repeat(8)));
        assert_eq!(rows[1], format!("  {}", "a".repeat(8)));
        assert_eq!(rows[2], format!("  {}", "a".repeat(4)));
    }

    #[test]
    fn a_word_straddling_a_style_boundary_keeps_each_half_styled_when_wrapped() {
        // TF-613: `Word` exists specifically so a single word split across
        // two differently-styled spans (e.g. `**bold**italic` straddling
        // mid-word, which CommonMark renders as adjacent spans with no
        // space between them) keeps each half's own style when rebuilt,
        // rather than losing the boundary.
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let italic = Style::new().add_modifier(Modifier::ITALIC);
        let line = Line::from(vec![
            Span::raw("- "),
            Span::styled("bold", bold),
            Span::styled("italic", italic),
        ]);
        let width = 40; // wide enough that this doesn't also need to wrap

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert_eq!(rewrapped.len(), 1);
        let spans = &rewrapped[0].spans;
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "• bolditalic");
        assert!(
            spans
                .iter()
                .any(|s| s.style == bold && s.content.as_ref() == "bold"),
            "the bold half should keep its own style as its own span, got {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.style == italic && s.content.as_ref() == "italic"),
            "the italic half should keep its own style as its own span, got {spans:?}"
        );
    }

    #[test]
    fn an_empty_body_list_item_renders_just_the_marker_without_panicking() {
        // TF-613: a list item with nothing after the marker (`- ` alone)
        // must not panic and must still render the substituted bullet.
        let line = Line::from(vec![Span::raw("- ")]);
        let width = 20;

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert_eq!(rewrapped.len(), 1);
        let text: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "• ");
    }

    #[test]
    fn a_line_with_a_non_multiple_of_four_indent_is_not_treated_as_a_list_item() {
        // A 2-space indent doesn't match any nesting level `tui-markdown`
        // itself would ever produce (each level adds exactly 4 columns), so
        // it must be left untouched rather than misidentified as a list
        // item at some fractional nesting depth.
        let line = Line::from(vec![Span::raw("  - "), Span::raw("not really a list item")]);
        let width = 20;

        assert!(rewrap_list_item(&line, width).is_none());
    }

    #[test]
    fn falls_back_to_a_single_unwrapped_row_when_the_wrap_budget_is_too_narrow() {
        // TF-613 regression: deep nesting in a narrow pane must not degrade
        // into a spindly character-per-row hard-break — that's worse than
        // the ambiguity it fixes. The `•` marker alone (regardless of
        // indent) is never confused with a plain `-` continuation, so below
        // a minimum body-text budget the line should render as a single
        // un-rewrapped row with just the marker swapped, leaving
        // continuation wrapping to `Paragraph`'s own `Wrap` instead.
        let indent = " ".repeat(12); // 3 nesting levels (4 columns each)
        let line = Line::from(vec![
            Span::raw(format!("{indent}- ")),
            Span::raw("ddddd eeeee fffff ggggg"),
        ]);
        let width = 18; // budget = 18 - 14 (indent+marker) = 4 columns

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert_eq!(
            rewrapped.len(),
            1,
            "narrow-width fallback should not hard-break into many spindly rows"
        );
        let text: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.starts_with(&format!("{indent}• ")));
        assert!(text.contains("ddddd eeeee fffff ggggg"));
    }

    #[test]
    fn rewrapped_list_item_rows_keep_the_source_lines_style_and_alignment() {
        // TF-613 regression: `rewrap_list_item` rebuilds each row from
        // scratch, so it must carry the source `Line`'s own `style`/
        // `alignment` onto every resulting row itself — `to_owned_line`
        // (the fallback for non-list lines, right above) already does this;
        // the list-item path shouldn't be a less faithful copy than that.
        let line = Line::from(vec![Span::raw("- "), Span::raw("aaaaa bbbbb ccccc ddddd")])
            .style(Style::new().bg(Color::Blue))
            .alignment(Alignment::Right);
        let width = 12; // forces the body onto more than one row

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert!(rewrapped.len() > 1, "test setup should force a wrap");
        for row in &rewrapped {
            assert_eq!(row.style, Style::new().bg(Color::Blue));
            assert_eq!(row.alignment, Some(Alignment::Right));
        }
    }

    #[test]
    fn multiple_consecutive_spaces_in_a_list_item_body_are_preserved() {
        // TF-613 regression: the hand-rolled wrapper this file replaces
        // `Paragraph`'s own `Wrap { trim: false }` with must not be lossier
        // than what it replaces. `Wrap { trim: false }` preserves a
        // deliberate multi-space run (e.g. aligned columns inside inline
        // code); the word-tokenizer here must not collapse it to one space.
        let line = Line::from(vec![Span::raw("- "), Span::raw("a    b")]);
        let width = 40;

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        assert_eq!(rewrapped.len(), 1, "should fit on a single row at width 40");
        let text: String = rewrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "• a    b");
    }

    #[test]
    fn wide_characters_are_measured_by_display_width_not_char_count() {
        // TF-613 regression: word-wrap fit decisions must measure display
        // columns (what actually determines whether a row overflows the
        // terminal), not `char`/Unicode-scalar count. CJK ideographs and
        // most emoji are 2-columns-wide but 1 `char`; undercounting them
        // lets a row silently overflow `width`, which then forces
        // `Paragraph`'s own re-wrap — reintroducing, for wide-character
        // content, exactly the ambiguous-continuation bug this file exists
        // to prevent.
        // `tui-markdown` always renders the marker as its own dedicated
        // span, separate from the body text (see `rewrap_list_item`'s doc
        // comment) — mirror that shape here rather than a single merged
        // span, which the false-positive guard would (correctly) reject.
        let line = Line::from(vec![
            Span::raw("- "),
            Span::raw("日本語のテキストがここにあります and trailing words here"),
        ]);
        let width = 40;

        let rewrapped = rewrap_list_item(&line, width).expect("a `- ` line is a list item");

        for row in &rewrapped {
            assert!(
                row.width() <= width,
                "row {row:?} is {} columns wide, exceeding the {width}-column budget",
                row.width()
            );
        }
    }

    #[test]
    fn code_block_lines_starting_with_a_hyphen_are_not_treated_as_list_markers() {
        // TF-613 false-positive guard: `rewrap_list_item` identifies a list
        // marker purely from the *shape* of a rendered line (leading
        // 4-space-multiple indent + literal `- `), which a fenced code
        // block's own content — e.g. a diff hunk — can coincidentally match.
        // A `- ` line inside a ```diff block is diff syntax, not a list
        // item, and must not have its `-` swapped for `•`.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-9",
            "```diff\n- old line here\n+ new line here\n```",
        )]);

        let text = rendered_text_with_size(&app, 100, 30);
        assert!(text.contains("- old line here"));
        assert!(!text.contains("• old line here"));
    }

    #[test]
    fn escaped_hyphen_paragraph_is_not_treated_as_a_list_marker() {
        // TF-613 false-positive guard: an escaped hyphen (`\-`) renders as a
        // literal `- ` at the start of a plain paragraph line, which must
        // not be mistaken for a list item's marker either.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue_with_description(
            "ENG-10",
            "\\- not a list item",
        )]);

        let text = rendered_text_with_size(&app, 100, 30);
        assert!(text.contains("- not a list item"));
        assert!(!text.contains("• not a list item"));
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
    fn shows_the_active_presets_name_in_the_list_title() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_active_preset(Some(ActivePreset {
            index: 0,
            name: "Urgent".to_string(),
        }));

        let text = rendered_text(&app);

        assert!(text.contains("My Issues [Urgent]"));
    }

    #[test]
    fn omits_the_preset_bracket_when_no_preset_is_active() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);

        let text = rendered_text(&app);

        assert!(text.contains("My Issues"));
        // No `[...]` bracket directly appended to the title — `[ ]`/`[x]` multi-select
        // markers on the issue rows themselves are unrelated and expected.
        assert!(!text.contains("My Issues ["));
    }

    #[test]
    fn shows_both_the_active_presets_name_and_the_live_filter_query() {
        // TF-647 AC: presets are independent of the live `/`-filter — it still layers on
        // top of whichever (preset or default_query) is active.
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        app.set_active_preset(Some(ActivePreset {
            index: 0,
            name: "Urgent".to_string(),
        }));
        handle_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "eng".chars() {
            handle_key(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // Wider than the default 60 columns — "My Issues [Urgent] — filter: eng" doesn't
        // fit the default-size list pane and the title would get truncated by the border.
        let text = rendered_text_with_size(&app, 100, 20);

        assert!(text.contains("My Issues [Urgent] — filter: eng"));
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

    #[test]
    fn about_lines_include_version_repo_and_license() {
        let text = about_lines().join("\n");

        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("github.com/talent-factory/herdr-linear"));
        assert!(text.contains("MIT"));
    }

    #[test]
    fn keybindings_lines_include_every_binding_from_the_registry() {
        let text = keybindings_lines().join("\n");

        for binding in crate::plugin::keybindings::KEYBINDINGS {
            assert!(
                text.contains(binding.keys),
                "missing key `{}` in keybindings tab",
                binding.keys
            );
            assert!(
                text.contains(binding.action),
                "missing action `{}` in keybindings tab",
                binding.action
            );
        }
    }

    #[test]
    fn keybindings_lines_group_by_context_with_headings() {
        let lines = keybindings_lines();

        assert!(lines.contains(&"Menu:".to_string()));
        assert!(lines.contains(&"Global:".to_string()));
    }

    #[test]
    fn keybindings_lines_does_not_repeat_a_context_heading() {
        let lines = keybindings_lines();
        let heading_count = lines.iter().filter(|line| *line == "Menu:").count();

        assert_eq!(heading_count, 1);
    }

    #[test]
    fn word_wrapped_row_count_is_one_for_an_empty_line() {
        assert_eq!(word_wrapped_row_count("", 28), 1);
    }

    #[test]
    fn word_wrapped_row_count_is_one_when_the_line_fits_within_width() {
        assert_eq!(word_wrapped_row_count("a short line", 28), 1);
    }

    #[test]
    fn word_wrapped_row_count_wraps_at_a_word_boundary_not_mid_word() {
        // "one two three" is 13 chars; at width 8, "one two" (7 chars) fits but adding
        // "three" wouldn't (7 + 1 + 5 = 13 > 8), so it must wrap before "three", not
        // mid-word.
        assert_eq!(word_wrapped_row_count("one two three", 8), 2);
    }

    #[test]
    fn word_wrapped_row_count_hard_breaks_a_single_word_wider_than_width() {
        let word = "a".repeat(65);
        assert_eq!(word_wrapped_row_count(&word, 28), 3); // ceil(65 / 28) == 3
    }

    #[test]
    fn word_wrapped_row_count_continues_correctly_after_a_hard_broken_word() {
        let line = format!("short {}", "a".repeat(65));
        // "short" (row 1) then the 65-char word can't fit in the remaining space, so it
        // starts its own fresh row and needs ceil(65/28) = 3 rows of its own: 1 + 3 = 4.
        assert_eq!(word_wrapped_row_count(&line, 28), 4);
    }

    /// Follow-up review fix (TF-585): `content_line_count` used to be the raw
    /// `Vec&lt;String&gt;` entry count, which under-counts whenever a tab has a line wider
    /// than the popup — the exact scenario that made the scroll clamp stop short of a
    /// tab's real end. Confirms the fix: a single very long entry now contributes more
    /// than one row to the estimate, not one.
    #[test]
    fn content_line_count_accounts_for_wrapping_of_a_long_line() {
        let long_line = "word ".repeat(40); // far wider than any realistic popup content

        let wrapped_rows = word_wrapped_row_count(&long_line, CONSERVATIVE_WRAP_WIDTH);

        assert!(
            wrapped_rows > 1,
            "expected wrapping to inflate the row count beyond the raw entry count of 1"
        );
    }

    /// Mirrors [`content_line_count_accounts_for_wrapping_of_a_long_line`]'s guard, for
    /// the Detail pane: a single long description paragraph must inflate
    /// [`detail_line_count`]'s estimate well beyond a short one, not just contribute the
    /// same one row regardless of length.
    #[test]
    fn detail_line_count_accounts_for_wrapping_of_a_long_description() {
        let short = sample_issue_with_description("ENG-1", "short");
        let long_description = "word ".repeat(80); // far wider than any realistic pane width
        let long = sample_issue_with_description("ENG-1", &long_description);

        assert!(
            detail_line_count(&long) > detail_line_count(&short),
            "a long description must inflate the row count beyond a short one"
        );
    }

    #[test]
    fn detail_line_count_counts_the_fixed_header_lines_even_with_an_empty_description() {
        let issue = sample_issue("ENG-1");

        // identifier, blank, title, blank, Status, Assignee, Project, blank — 8 header
        // lines, each contributing at least one row even before any description content.
        assert!(detail_line_count(&issue) >= 8);
    }

    /// Regression test (PR #44 review): pins down `DETAIL_CONSERVATIVE_WRAP_WIDTH`'s own
    /// "narrower-than-real is safe" margin directly, rather than trusting its doc comment's
    /// arithmetic to stay honest on its own — which is exactly how the original `20`
    /// (zero margin below its claimed 44-column/20-column floor) shipped unnoticed. At a
    /// 40-column terminal (real Detail-pane content width `40 / 2 - 2 = 18`, narrower than
    /// the 44-column terminal the doc calls out), `detail_line_count`'s estimate — built
    /// from the fixed conservative width, oblivious to the real one — must still be at
    /// least as large as what a real render at that narrower width actually needs. A
    /// constant equal to or wider than the real floor would under-count here instead,
    /// exactly the "`j`/wheel clamps short of the true end" failure this invariant exists
    /// to prevent.
    #[test]
    fn detail_line_count_estimate_stays_at_or_above_the_real_wrapped_row_count_at_a_narrow_width() {
        const NARROWEST_SUPPORTED_REAL_WIDTH: usize = 18; // a 40-column terminal's Detail pane

        let long_description = "word ".repeat(80); // forces real wrapping at either width
        let issue = sample_issue_with_description("ENG-1", &long_description);

        let estimate = detail_line_count(&issue);
        let real_rows: usize = build_detail_lines(&issue, NARROWEST_SUPPORTED_REAL_WIDTH)
            .iter()
            .map(|line| {
                word_wrapped_row_count(&line_plain_text(line), NARROWEST_SUPPORTED_REAL_WIDTH)
            })
            .sum();

        assert!(
            estimate >= real_rows,
            "estimate ({estimate}) must stay at or above the real render's row count \
             ({real_rows}) at the narrowest supported terminal width"
        );
    }

    /// Regression test (PR #44 review): every other `App::detail_scroll` test asserts on
    /// `App` state alone, never on what actually reaches the screen — so a wiring break in
    /// `draw_view` (a swapped `.scroll((0, *detail_scroll))` instead of
    /// `.scroll((*detail_scroll, 0))`, or the `.scroll(...)` call dropped entirely) would
    /// pass every one of them undetected despite the Detail pane silently never actually
    /// scrolling for the user. Renders a description long enough that its first and last
    /// paragraphs can't both fit on screen at once, and confirms scrolling to the real
    /// rendered end moves the *rendered* content, not just the stored offset.
    ///
    /// Scrolls by the *real* row count at the render's actual width (60 columns → a
    /// 28-column Detail pane), not `detail_line_count`'s conservative estimate (built for
    /// a narrower, unrelated assumed width — see `detail_line_count_estimate_stays_at_or_
    /// above_the_real_wrapped_row_count_at_a_narrow_width` for that invariant on its own).
    /// Scrolling to the *estimate*'s max here would overshoot past this test's real,
    /// wider-than-assumed content, landing on blank space past the end rather than on the
    /// last real line — `Paragraph::scroll` doesn't clamp an offset past its own content,
    /// it just renders nothing.
    #[test]
    fn detail_scroll_offset_reaches_the_rendered_detail_pane() {
        let mut app = app_in_my_issues_view();
        let filler = "filler paragraph\n\n".repeat(28);
        let description = format!("ALPHA-TOP-MARKER\n\n{filler}ZULU-BOTTOM-MARKER");
        app.set_issues(vec![sample_issue_with_description("ENG-1", &description)]);

        let before = rendered_text_with_size(&app, 60, 15);
        assert!(
            before.contains("ALPHA-TOP-MARKER"),
            "the first description paragraph must be visible before any scrolling"
        );
        assert!(
            !before.contains("ZULU-BOTTOM-MARKER"),
            "the last description paragraph must not already be visible before scrolling"
        );

        // The Detail pane's real content width at a 60-column terminal: half the frame
        // (a 50/50 split, even so no ceil/floor asymmetry) minus 2 columns for its
        // bordered `Block` — mirrors `DETAIL_CONSERVATIVE_WRAP_WIDTH`'s own formula.
        const REAL_DETAIL_WIDTH: usize = 60 / 2 - 2;
        let issue = app.selected_issue().unwrap();
        let real_row_count: usize = build_detail_lines(issue, REAL_DETAIL_WIDTH)
            .iter()
            .map(|line| word_wrapped_row_count(&line_plain_text(line), REAL_DETAIL_WIDTH))
            .sum();
        for _ in 0..real_row_count {
            app.detail_scroll_down(real_row_count);
        }

        let after = rendered_text_with_size(&app, 60, 15);
        assert!(
            !after.contains("ALPHA-TOP-MARKER"),
            "the first description paragraph must have scrolled out of view"
        );
        assert!(
            after.contains("ZULU-BOTTOM-MARKER"),
            "scrolling to the clamped end must bring the last description paragraph into view"
        );
    }

    #[test]
    fn extract_section_after_returns_entries_between_heading_and_next_section() {
        let changelog = "## [Unreleased]\n### Added\n- Thing one\n- Thing two\n\n\
                         ## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        let section = extract_section_after(changelog, "## [Unreleased]").unwrap();

        assert_eq!(
            section,
            vec![
                "### Added".to_string(),
                "- Thing one".to_string(),
                "- Thing two".to_string(),
            ]
        );
    }

    #[test]
    fn extract_section_after_stops_at_the_next_level_two_heading() {
        let changelog = "## [Unreleased]\n### Added\n- Thing\n\
                         ## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        let section = extract_section_after(changelog, "## [Unreleased]").unwrap();

        assert!(!section.iter().any(|line| line.contains("Old thing")));
    }

    #[test]
    fn extract_section_after_returns_none_for_an_empty_section() {
        let changelog = "## [Unreleased]\n\n## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        assert_eq!(extract_section_after(changelog, "## [Unreleased]"), None);
    }

    #[test]
    fn extract_section_after_returns_none_when_heading_is_missing() {
        let changelog = "## [0.1.0] - 2026-08-04\n### Added\n- Old thing\n";

        assert_eq!(extract_section_after(changelog, "## [Unreleased]"), None);
    }

    #[test]
    fn whats_new_lines_from_uses_unreleased_entries_when_present() {
        let changelog = "## [Unreleased]\n### Added\n- Thing one\n\n\
                         ## [0.1.0] - 2026-08-04\n### Added\n- Old\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines[0].contains(env!("CARGO_PKG_VERSION")));
        assert!(lines.contains(&"### Added".to_string()));
        assert!(lines.contains(&"- Thing one".to_string()));
        assert!(!lines.iter().any(|line| line.contains("- Old")));
    }

    #[test]
    fn whats_new_lines_from_falls_back_to_the_next_release_when_unreleased_is_empty() {
        let changelog = "## [Unreleased]\n\n## [0.1.0] - 2026-08-04\n### Added\n- First release\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines.contains(&"- First release".to_string()));
    }

    #[test]
    fn whats_new_lines_from_falls_back_when_unreleased_heading_is_missing_entirely() {
        let changelog = "## [0.1.0] - 2026-08-04\n### Added\n- First release\n";

        let lines = whats_new_lines_from(changelog);

        assert!(lines.contains(&"- First release".to_string()));
    }

    #[test]
    fn whats_new_lines_from_reports_when_nothing_parses_at_all() {
        let changelog = "Not a changelog at all.";

        let lines = whats_new_lines_from(changelog);

        assert!(lines
            .iter()
            .any(|line| line.contains("Couldn't find recent changes")));
    }

    /// Follow-up review fix (TF-585): `whats_new_lines_from` used to show the entire
    /// `[Unreleased]` section verbatim, however large — confirms an oversized section
    /// now gets truncated to `WHATS_NEW_MAX_LINES` with a visible "…N more" notice
    /// instead of dumping everything into the overlay.
    #[test]
    fn whats_new_lines_from_truncates_a_very_long_unreleased_section_with_a_notice() {
        let entries = (0..50)
            .map(|i| format!("- entry {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let changelog = format!("## [Unreleased]\n### Added\n{entries}\n");

        let lines = whats_new_lines_from(&changelog);

        assert_eq!(lines.len(), WHATS_NEW_MAX_LINES);
        let last = lines.last().unwrap();
        assert!(
            last.contains("more line") && last.contains("CHANGELOG.md"),
            "expected a truncation notice, got: {last:?}"
        );
        // The entries that *did* make the cut must be the first ones, not an arbitrary
        // subset — a reader scanning from the top should see the earliest (per
        // Keep-a-Changelog convention, most-recently-added) entries first.
        assert!(lines.contains(&"- entry 0".to_string()));
        assert!(!lines.contains(&"- entry 49".to_string()));
    }

    #[test]
    fn whats_new_lines_from_does_not_truncate_when_within_the_limit() {
        let changelog = "## [Unreleased]\n### Added\n- one\n- two\n\
                         ## [0.1.0] - 2026-08-04\n### Added\n- old\n";

        let lines = whats_new_lines_from(changelog);

        assert!(!lines.iter().any(|line| line.contains("more line")));
    }

    #[test]
    fn truncate_with_notice_is_a_no_op_when_already_within_the_limit() {
        let mut lines = vec!["a".to_string(), "b".to_string()];

        truncate_with_notice(&mut lines, 5);

        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn truncate_with_notice_reports_the_correct_hidden_count() {
        let mut lines: Vec<String> = (0..10).map(|i| i.to_string()).collect();

        truncate_with_notice(&mut lines, 4);

        assert_eq!(lines.len(), 4);
        assert!(lines[3].contains("7 more lines"), "got: {:?}", lines[3]);
    }

    #[test]
    fn whats_new_lines_reads_the_real_changelog_and_includes_the_current_version() {
        let lines = whats_new_lines();

        assert!(lines[0].contains(env!("CARGO_PKG_VERSION")));
        assert!(lines.len() > 2, "expected real entries, got: {lines:?}");
        // Follow-up review fix (TF-585): the real `[Unreleased]` section runs well past
        // 100 lines (everything since the project's only release, not just recent
        // work) — this is the ceiling the old version of this test was missing, which
        // is exactly why the lack of a size cap went unnoticed.
        assert!(
            lines.len() <= WHATS_NEW_MAX_LINES,
            "expected the What's New tab to stay within its display cap, got {} lines: {lines:?}",
            lines.len()
        );
    }

    #[test]
    fn settings_lines_from_not_found_shows_defaults_message() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::NotFound,
            api_key_set: false,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: std::collections::BTreeMap::new(),
            default_query: None,
            filter_presets: Vec::new(),
        };

        let lines = settings_lines_from(&summary, false).join("\n");

        assert!(lines.contains("no file found, using defaults"));
        assert!(lines.contains("✗ Not set"));
        assert!(lines.contains("(default)"));
        assert!(lines.contains("default_query    = Not set"));
    }

    /// Follow-up review fix (TF-585): `settings_lines()` used to collapse "LINEAR_API_KEY
    /// unset" and "LINEAR_API_KEY set but not valid UTF-8" into the same "✗ Not set" —
    /// misleading specifically on the one tab whose job is explaining *why* a key isn't
    /// resolving. Confirms the two cases now render distinct messages given the same
    /// otherwise-empty `ResolvedConfigSummary`.
    #[test]
    fn settings_lines_from_distinguishes_unset_from_non_utf8_env_key() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::NotFound,
            api_key_set: false,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: std::collections::BTreeMap::new(),
            default_query: None,
            filter_presets: Vec::new(),
        };

        let unset = settings_lines_from(&summary, false).join("\n");
        let non_utf8 = settings_lines_from(&summary, true).join("\n");

        assert!(unset.contains("✗ Not set"));
        assert!(!unset.contains("UTF-8"));
        assert!(non_utf8.contains("✗ Not set"));
        assert!(non_utf8.contains("LINEAR_API_KEY is set but isn't valid UTF-8"));
    }

    #[test]
    fn settings_lines_from_found_shows_masked_api_key_and_resolved_values() {
        let mut project_overrides = std::collections::BTreeMap::new();
        project_overrides.insert("herdr-linear".to_string(), "proj-1".to_string());
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::Found,
            api_key_set: true,
            agent_command: Some("my-agent".to_string()),
            team_id: Some("team-123".to_string()),
            editor: Some("vim".to_string()),
            project_overrides,
            default_query: Some("priority:>=2".to_string()),
            filter_presets: vec![crate::plugin::config::FilterPreset {
                name: "Urgent".to_string(),
                query: "priority:>=2".to_string(),
            }],
        };

        let lines = settings_lines_from(&summary, false).join("\n");

        assert!(lines.contains("Config: found"));
        assert!(lines.contains("✓ Set"));
        assert!(!lines.contains("lin_api_"));
        assert!(lines.contains("my-agent"));
        assert!(lines.contains("editor           = vim"));
        assert!(lines.contains("team-123"));
        assert!(lines.contains("default_query    = priority:>=2"));
        assert!(lines.contains("herdr-linear"));
        assert!(lines.contains("proj-1"));
        assert!(lines.contains("filter_presets:"));
        assert!(lines.contains("Urgent"));
        assert!(lines.contains("priority:>=2"));
    }

    #[test]
    fn settings_lines_from_found_shows_none_for_no_filter_presets() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::Found,
            api_key_set: true,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: std::collections::BTreeMap::new(),
            default_query: None,
            filter_presets: Vec::new(),
        };

        let lines = settings_lines_from(&summary, false).join("\n");

        assert!(lines.contains("filter_presets: (none)"));
    }

    #[test]
    fn settings_lines_from_invalid_shows_the_error_message_and_no_stale_values() {
        let summary = crate::plugin::config::ResolvedConfigSummary {
            path: "/fake/config.toml".to_string(),
            status: crate::plugin::config::ConfigFileStatus::Invalid("not valid TOML".to_string()),
            api_key_set: false,
            agent_command: None,
            team_id: None,
            editor: None,
            project_overrides: std::collections::BTreeMap::new(),
            default_query: None,
            filter_presets: Vec::new(),
        };

        let lines = settings_lines_from(&summary, false).join("\n");

        assert!(lines.contains("is invalid"));
        assert!(lines.contains("not valid TOML"));
        assert!(lines.contains("✗ Not set"));
    }

    #[test]
    fn help_overlay_renders_on_top_of_the_menu_when_open() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("Help:"));
        assert!(text.contains("What's New"));
        assert!(text.contains("Keybindings"));
        assert!(text.contains("Settings"));
        assert!(text.contains("About"));
    }

    /// Follow-up review fix (TF-585): the active tab used to be marked with a plain
    /// "> " text prefix, which user testing on the running app found too easy to miss at
    /// a glance. It's now a reversed-video highlight instead (matching `draw_menu`'s own
    /// selection style) — this asserts the *actual visual style*, not just that some text
    /// marker string is present, since a plain `rendered_text`-based `.contains()` check
    /// can't tell a styled render from an unstyled one at all (style info doesn't survive
    /// that helper's flattening to a plain string).
    #[test]
    fn help_overlay_marks_the_active_tab_with_a_reversed_highlight() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE); // -> WhatsNew (default)

        let cells = rendered_cells_with_size(&app, 100, 30);
        let symbols: Vec<String> = cells.iter().map(|(s, _)| s.clone()).collect();

        let active_start = find_cell_run(&symbols, "What's New")
            .expect("expected to find the active tab's label in the render");
        for cell in &cells[active_start..active_start + "What's New".chars().count()] {
            assert!(
                cell.1.contains(Modifier::REVERSED),
                "expected every cell of the active tab's label to be reversed-highlighted, \
                 got symbol {:?} with modifier {:?}",
                cell.0,
                cell.1
            );
        }

        // An *inactive* tab's label must NOT carry the same highlight, or every tab
        // would look "selected" and the highlight would communicate nothing.
        let inactive_start = find_cell_run(&symbols, "About")
            .expect("expected to find an inactive tab's label in the render");
        for cell in &cells[inactive_start..inactive_start + "About".chars().count()] {
            assert!(
                !cell.1.contains(Modifier::REVERSED),
                "expected an inactive tab's label to NOT be reversed-highlighted, \
                 got symbol {:?} with modifier {:?}",
                cell.0,
                cell.1
            );
        }
    }

    #[test]
    fn help_overlay_shows_the_footer_controls() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        let text = rendered_text_with_size(&app, 100, 30);

        // Follow-up review fix (TF-585): the original assertion here only checked for
        // "close", which would still pass even if the switch/jump/scroll hints were
        // dropped or garbled. Assert the full footer text verbatim so a change to any
        // part of it is caught, not just an accidental removal of the whole line.
        assert!(
            text.contains("Tab/←→ switch · 1-4 jump · j/k scroll · Esc/q/? close"),
            "footer controls text missing or changed, got: {text:?}"
        );
    }

    #[test]
    fn help_overlay_switches_tab_content_on_number_jump() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('4'), KeyModifiers::NONE); // -> About

        let text = rendered_text_with_size(&app, 100, 30);

        assert!(text.contains("About"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

    /// Follow-up review fix (TF-585): the About tab was the only one ever rendered
    /// end-to-end through `handle_key` + `rendered_text_with_size` — Keybindings and
    /// Settings (below) were exercised only at the `*_lines()`/`*_lines_from()` function
    /// level, never through the full `draw_help_overlay` render path. This confirms the
    /// Keybindings tab actually renders every entry from the canonical registry, not
    /// just that `keybindings_lines()` in isolation contains them.
    #[test]
    fn help_overlay_renders_the_keybindings_tab_with_every_registered_binding() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE); // -> Keybindings

        let text = rendered_text_with_size(&app, 100, 40);

        assert!(text.contains("Keybindings"));
        for binding in crate::plugin::keybindings::KEYBINDINGS {
            assert!(
                text.contains(binding.action),
                "rendered Keybindings tab is missing action `{}`",
                binding.action
            );
        }
    }

    /// Companion to the above for the Settings tab — the one tab whose content is
    /// produced by `settings_lines()`, the impure wrapper that reads
    /// `HERDR_PLUGIN_CONFIG_DIR`/`LINEAR_API_KEY` from the real environment (previously
    /// never called by any test; only its pure half `settings_lines_from` was, with
    /// injected data). Deliberately doesn't set or clear those env vars — mutating them
    /// would risk interfering with other tests running in parallel in the same process
    /// (this crate's established convention throughout `config.rs` is to leave impure
    /// env-reading wrappers untested for exactly that reason) — so this only asserts the
    /// field labels `settings_lines_from` always emits regardless of what's actually
    /// resolved, which is enough to confirm `settings_lines()` really is wired through to
    /// a real render, not just its pure half in isolation.
    #[test]
    fn help_overlay_renders_the_settings_tab_via_the_real_config_wiring() {
        let mut app = App::new();
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        handle_key(&mut app, KeyCode::Char('3'), KeyModifiers::NONE); // -> Settings

        let text = rendered_text_with_size(&app, 100, 40);

        assert!(text.contains("Settings"));
        assert!(text.contains("Location:"));
        assert!(text.contains("api_key"));
        assert!(text.contains("agent_command"));
        assert!(text.contains("team_id"));
        assert!(text.contains("project_overrides"));
    }

    #[test]
    fn help_overlay_closes_and_the_underlying_screen_reappears_unchanged() {
        let mut app = app_in_my_issues_view();
        app.set_issues(vec![sample_issue("ENG-1")]);
        handle_key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);

        handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        let text = rendered_text(&app);
        assert!(!text.contains("Help:"));
        assert!(text.contains("ENG-1"));
    }
}
