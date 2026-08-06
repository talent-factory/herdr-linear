# Detail-Pane Markdown Rendering + Clickable URL — design

**Date:** 2026-08-05
**Status:** Approved
**Linear issue:** [TF-583](https://linear.app/talent-factory/issue/TF-583)

## Problem

The Detail pane (`src/plugin/ui.rs::draw_view`, `ViewState::Loaded` branch) renders a single
hand-built `Paragraph::new(format!(...))` string: identifier, title, state, URL. `issue.description`
(Linear-authored Markdown) is never shown — to read it, the user has to copy `issue.url` and open it
in a browser manually. `issue.description` is already fetched by the GraphQL queries in
`src/queries.rs`, so this is a pure rendering change, no backend work.

## Scope

- Render `issue.description` as formatted Markdown (headings, lists, code fences, bold/italic
  visually distinct — not raw `#`/`*`/`` ` ``) in the Detail pane.
- Make the displayed `issue.url` clickable in terminals that support OSC 8 hyperlinks, opening the
  issue in the default browser without retyping/copying the URL.
- A unit/snapshot-style test in `src/plugin/ui.rs` covering description rendering, mirroring the
  existing `renders_issue_identifier_and_title_in_the_list` pattern.

Out of scope: scrolling the description body (pane already has no scroll state; long content wraps
and clips at the pane height today, unchanged by this work), syntax-highlighted code blocks (visual
distinction is enough per the AC), and terminal-capability detection for OSC 8 (see "Hyperlink
technique" below — unsupported terminals degrade gracefully with no detection needed). The existing
`o` keybinding (`Action::OpenInBrowser` → `open::that(url)`) is untouched — it stays as a
keyboard-only complement to the now-clickable URL text.

## Markdown rendering: `tui-markdown`

Add the `tui-markdown` crate (`default-features = false`, skipping its `syntect`-based
syntax-highlighting feature — heavier dependency, not required by the AC) gated into the existing
`plugin` Cargo feature, alongside `ratatui`/`crossterm`/`open`. `tui_markdown::from_str(&description)`
converts the Markdown `&str` directly into a `ratatui::text::Text`, which the Detail `Paragraph`
renders as today — no hand-written parser to maintain.

`tui-markdown` v0.3.9 depends on `ratatui-core ^0.1`, already satisfied by the `ratatui-core 0.1.2`
pinned in `Cargo.lock` — no version conflict. Its MSRV is `1.88.0`, above this project's declared
`rust-version = "1.70"`; bump `rust-version` to `"1.88"` in `Cargo.toml`. CI (`.github/workflows/ci.yml`)
only runs `stable/beta/nightly`, no pinned-MSRV job, and the README makes no MSRV claim — the bump is
a paper change with no CI impact.

An empty/`None` `issue.description` renders as an empty body (no heading, no placeholder text) —
matches how the current code already treats a missing description implicitly via `format!`.

## Hyperlink technique: post-render OSC 8 cell overlay

Detail pane layout changes from one `Paragraph` to three vertically stacked pieces within
`chunks[1]`:

1. **Header** — identifier, title, state (unchanged plain text, same as today).
2. **Description body** — the `tui-markdown`-rendered `Text`, takes the remaining space.
3. **URL footer** — one fixed-height line at the bottom, `format!("URL: {url}")` as plain text.

The fixed-position footer makes the hyperlink overlay simple: a small `Hyperlink` widget (new,
`src/plugin/ui.rs` or a submodule) implements `Widget for &Hyperlink`, mirroring ratatui's own
upstream `examples/hyperlink.rs` pattern:

1. Render the line as normal text first (`Paragraph`/`Line` into the `Buffer`) — plain-text width,
   wrapping, and existing/new tests all operate on this unchanged.
2. Walk the cells covering the visible URL text; wrap the **first** cell's symbol with the OSC 8
   opening escape (`\x1b]8;;{url}\x1b\\`) prefix and the **last** cell's symbol with the closing
   escape (`\x1b]8;;\x1b\\`) suffix, leaving cells in between untouched. Because ratatui's terminal
   backend coalesces adjacent same-style cells into one `Print` call, this reassembles into a single
   correctly-escaped write; because OSC 8 bytes are zero-width to the terminal, cursor advancement
   and layout are unaffected.

No capability detection: terminals without OSC 8 support ignore the unrecognized escape per spec and
render the plain URL text, same as today — matching how `git`/`bat`/`ls --hyperlink` already behave
unconditionally. The `Hyperlink` widget's escape-wrapping is unit-tested directly against the exact
byte sequence it produces (independent of a real terminal), decoupled from the ratatui `TestBackend`
snapshot tests.

## Testing strategy

- `ui.rs`: new test (name TBD in the plan, mirrors `renders_issue_identifier_and_title_in_the_list`)
  asserting a sample issue with a Markdown description (heading + list item) renders those elements
  distinctly — e.g. the heading text appears without a literal leading `#`, list marker rendered, not
  raw `-`/`*` — using the existing `rendered_text(&app)` `TestBackend` helper.
- New `Hyperlink` widget: unit test(s) asserting the exact OSC 8-wrapped string produced for a known
  URL/width, no terminal required.
- No change to `plugin::repo`/`plugin::config`/`client.rs` — this phase is UI-only.

## Out of scope / open items for the implementation plan

- Exact test name(s) and the specific sample Markdown fixture used — implementation-plan detail, not
  a design-level decision.
- Whether the `Hyperlink` widget lives inline in `ui.rs` or a new `src/plugin/hyperlink.rs` module —
  implementation-plan detail; lean toward inline first given its small size, split out only if it
  grows.
