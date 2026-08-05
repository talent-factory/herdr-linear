# Detail-Pane Markdown Rendering + Clickable URL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render `issue.description` as formatted Markdown and make `issue.url` a clickable OSC 8 terminal hyperlink in the Detail pane (`src/plugin/ui.rs`).

**Architecture:** Split the Detail pane's inner area into three vertically stacked pieces (header / markdown body / URL footer) instead of one combined `Paragraph`. `tui_markdown::from_str` converts `issue.description` into a `ratatui::text::Text` for the body. A new pure `Hyperlink` widget renders the footer line normally, then overlays OSC 8 escape bytes onto the already-rendered `Buffer` cells covering the URL substring — zero-width to the terminal, so layout/width/wrap and existing plain-text tests are unaffected.

**Tech Stack:** Rust, `ratatui` 0.30.2, `tui-markdown` 0.3.9 (new), `crossterm` 0.29.0.

## Global Constraints

- `rust-version` in `Cargo.toml` moves from `"1.70"` to `"1.88"` (required by `tui-markdown`'s MSRV). No CI job pins MSRV, so this is safe.
- `tui-markdown` is added with `default-features = false` (skip the `syntect`-based `highlight-code` feature — not required by the AC).
- `tui-markdown` is gated into the existing `plugin` Cargo feature, same as `ratatui`/`crossterm`/`open`.
- No new runtime dependency for OSC 8 support detection — unsupported terminals ignore the escape and show plain text, matching `git`/`bat`/`ls --hyperlink` behavior.
- No scrolling is added to the Detail pane (out of scope, matches current no-scroll behavior).
- The existing `o` keybinding (`Action::OpenInBrowser`) is untouched.

---

## File Structure

- **Modify: `Cargo.toml`** — bump `rust-version`, add the `tui-markdown` dependency, add it to the `plugin` feature list.
- **Modify: `src/plugin/ui.rs`** — add the `Hyperlink` widget (private, module-level), rewrite `draw_view`'s `ViewState::Loaded` detail-pane rendering, extend the test module (new fixtures + new tests).

No other files change — this is a UI-only rendering change; `issue.description` is already fetched by the existing GraphQL queries.

---

## Task 1: Add `tui-markdown` dependency, bump MSRV

**Files:**
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: the `tui-markdown` crate available (behind the `plugin` feature) for Task 3 to call `tui_markdown::from_str(&str) -> ratatui::text::Text`.

- [ ] **Step 1: Edit `Cargo.toml`**

Change line 5 from:

```toml
rust-version = "1.70"
```

to:

```toml
rust-version = "1.88"
```

Change the "Plugin binary only" dependency block (currently lines 24-28) from:

```toml
# Plugin binary only (enabled via the `plugin` feature)
ratatui = { version = "0.30.1", optional = true }
crossterm = { version = "0.29.0", optional = true }
toml = { version = "1.1", default-features = false, features = ["parse", "serde"], optional = true }
open = { version = "5", optional = true }
```

to:

```toml
# Plugin binary only (enabled via the `plugin` feature)
ratatui = { version = "0.30.1", optional = true }
crossterm = { version = "0.29.0", optional = true }
toml = { version = "1.1", default-features = false, features = ["parse", "serde"], optional = true }
open = { version = "5", optional = true }
tui-markdown = { version = "0.3", default-features = false, optional = true }
```

Change the `plugin` feature (currently line 37) from:

```toml
plugin = ["ratatui", "crossterm", "toml", "open"]
```

to:

```toml
plugin = ["ratatui", "crossterm", "toml", "open", "tui-markdown"]
```

- [ ] **Step 2: Verify the dependency resolves and the crate builds**

Run: `cargo build --features plugin`
Expected: builds successfully (downloads `tui-markdown` and its transitive deps — `pulldown-cmark`, `ansi-to-tui`, `itertools`, etc. — into `Cargo.lock`).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tui-markdown dependency, bump MSRV to 1.88"
```

---

## Task 2: `Hyperlink` widget — OSC 8 cell overlay (pure, unit-tested)

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: nothing beyond `ratatui` types already available after Task 1's `cargo build`.
- Produces: `struct Hyperlink<'a> { .. }` with `Hyperlink::new(text: ratatui::text::Line<'a>, url: impl Into<String>) -> Hyperlink<'a>`, and `impl Widget for &Hyperlink<'_>`. Task 3 renders it via `frame.render_widget(&hyperlink, area)`. The widget wraps the **trailing** `url.chars().count()` cells of the rendered `text` line in OSC 8 escapes (the URL is always the suffix of the line, e.g. `"URL: {url}"`).

- [ ] **Step 1: Write the failing tests**

Add to the bottom of the existing `#[cfg(test)] mod tests { ... }` block in `src/plugin/ui.rs` (after the last existing test, before the closing `}`):

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `cargo test --features plugin --lib plugin::ui::tests::hyperlink_`
Expected: compile error — `Hyperlink`, `Line`, `Buffer`, `Rect`, `Widget` not found in this scope (none exist yet).

- [ ] **Step 3: Add imports and implement `Hyperlink`**

Change the top-of-file `use ratatui::{ ... };` block from:

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
```

to:

```rust
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Widget, Wrap},
    Frame,
};
```

Add this new code after the `draw_menu` function and before `fn draw_view` (i.e. right after line 42's closing `}`):

```rust
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
        let start = total_width.saturating_sub(link_width).min(area.width as usize);
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --features plugin --lib plugin::ui::tests::hyperlink_`
Expected: both tests PASS.

- [ ] **Step 5: Run the full `ui` test suite to check for regressions**

Run: `cargo test --features plugin --lib plugin::ui`
Expected: all existing tests still PASS (the widget isn't wired into `draw_view` yet, so nothing else should change).

- [ ] **Step 6: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: add Hyperlink widget for OSC 8 terminal hyperlinks"
```

---

## Task 3: Render Markdown description + wire the `Hyperlink` footer into the Detail pane

**Files:**
- Modify: `src/plugin/ui.rs`

**Interfaces:**
- Consumes: `Hyperlink::new(text: Line<'a>, url: impl Into<String>) -> Hyperlink<'a>` and `impl Widget for &Hyperlink<'_>` from Task 2; `tui_markdown::from_str(&str) -> ratatui::text::Text` from the `tui-markdown` crate (Task 1); `Issue.description: Option<String>` and `Issue.url: String` from `crate::Issue`.
- Produces: the rewritten `draw_view` Detail-pane rendering (no new public interface — this is the top of the call chain for this feature).

- [ ] **Step 1: Refactor the issue test fixture to support a description, and add a wider-terminal render helper**

In `mod tests`, replace the existing `sample_issue` function:

```rust
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
```

with:

```rust
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
```

Replace the existing `rendered_text` function:

```rust
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
```

with:

```rust
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
```

- [ ] **Step 2: Run the existing test suite to confirm the refactor is behavior-preserving**

Run: `cargo test --features plugin --lib plugin::ui`
Expected: all existing tests still PASS (this step only renamed/split a helper — no behavior changed yet).

- [ ] **Step 3: Write the new failing tests**

Add to the bottom of `mod tests` (after the `Hyperlink` tests from Task 2):

```rust
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
        assert!(text.contains("URL: https://linear.app/team/issue/ENG-3"));
    }
```

- [ ] **Step 4: Run the new tests to verify they fail**

Run: `cargo test --features plugin --lib plugin::ui::tests::renders_issue_description_as_formatted_markdown cargo test --features plugin --lib plugin::ui::tests::renders_issue_url_in_the_detail_footer`
Expected: `renders_issue_description_as_formatted_markdown` FAILS (current `draw_view` never includes `issue.description`, so none of `"Heading"`/`"item one"`/`"bold"`/`"code"` appear). `renders_issue_url_in_the_detail_footer` may already PASS against the current single-`Paragraph` implementation — that's fine, it becomes a regression guard for the rewrite in the next step.

- [ ] **Step 5: Rewrite `draw_view`'s `ViewState::Loaded` detail-pane rendering**

Replace the `ViewState::Loaded { issues, selected } => { ... }` arm of `draw_view` (currently lines 61-91):

```rust
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
```

with:

```rust
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
                let body =
                    Paragraph::new(tui_markdown::from_str(description)).wrap(Wrap { trim: true });
                frame.render_widget(body, sections[1]);

                let hyperlink =
                    Hyperlink::new(Line::from(format!("URL: {}", issue.url)), issue.url.clone());
                frame.render_widget(&hyperlink, sections[2]);
            }
        }
```

- [ ] **Step 6: Run the full `ui` test suite**

Run: `cargo test --features plugin --lib plugin::ui`
Expected: every test in the module PASSES — the two Task 2 `Hyperlink` tests, the two new tests from Step 3, and all pre-existing tests (`renders_all_three_menu_options_on_start`, `marks_unavailable_menu_options_as_coming_soon`, `renders_loading_message`, `renders_error_message_with_retry_hint`, `renders_issue_identifier_and_title_in_the_list`).

- [ ] **Step 7: Commit**

```bash
git add src/plugin/ui.rs
git commit -m "feat: render issue description as Markdown, make URL clickable (TF-583)"
```

---

## Task 4: Full verification pass

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt --all -- --check`
Expected: no diff. If it reports formatting issues, run `cargo fmt --all`, review the diff is only whitespace, then `git add -A && git commit -m "chore: cargo fmt"`.

- [ ] **Step 2: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings/errors. Fix any that appear (most likely candidates: an unused import if an editing step above left one, or a clippy suggestion on the `match (bool, bool)` pattern in `Hyperlink::render` — e.g. clippy may prefer an `if`/`else if` chain; apply the suggested fix) and re-run until clean, then commit any fix separately (`git commit -m "chore: fix clippy warnings"`).

- [ ] **Step 3: Full test suite**

Run: `cargo test --all-features`
Expected: all tests pass, including the full `plugin::ui` module and every other existing test in the crate (unaffected by this change).

- [ ] **Step 4: Docs build**

Run: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
Expected: builds cleanly (matches the `doc` CI job).

- [ ] **Step 5: Manual acceptance-criteria check against the live TUI**

Run: `cargo run --features plugin` (or the project's usual plugin-launch flow, e.g. `just plugin-reinstall` if configured), open an issue with a Markdown description in "My Issues" or "Project Issues", and confirm:
- The description is visible and headings/lists/code/bold render distinctly, not as raw `#`/`-`/`` ` ``/`**`.
- The `URL: https://...` line is clickable in a terminal that supports OSC 8 (e.g. iTerm2, Windows Terminal, kitty) and opens the issue in the browser.
- `o` still opens the issue in the browser as before.

No commit for this step — it's a verification checklist, not a code change. If it surfaces a problem, fix it as a new commit and re-run Steps 1-4.

---

## Self-Review Notes

- **Spec coverage:** All four design-doc requirements are covered — description rendering (Task 3), distinct Markdown formatting via `tui-markdown` (Task 1 + 3), clickable URL via OSC 8 (Task 2 + 3), and a `ui.rs` test covering description rendering (Task 3, Step 3).
- **Type consistency:** `Hyperlink::new(text: Line<'a>, url: impl Into<String>)` defined in Task 2 is called identically in Task 3 (`Hyperlink::new(Line::from(...), issue.url.clone())`). `sample_issue`/`sample_issue_with_description`/`sample_issue_json` signatures introduced in Task 3 Step 1 are used consistently in Task 3 Step 3's new tests.
- **No placeholders:** every step contains complete, runnable code — no "add error handling" or "similar to Task N" placeholders.
