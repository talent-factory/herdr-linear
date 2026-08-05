# herdr-linear: Plugin View Switcher — Design

**Date:** 2026-08-05
**Status:** Approved for planning
**Linear:** [TF-576](https://linear.app/talent-factory/issue/TF-576)

## Purpose

The plugin (v1, see `2026-08-04-herdr-plugin-layer-design.md`) hard-wires its only view to
the authenticated user's assigned issues (`fetch_my_issues`). That design explicitly scoped
"team/project/cycle browsing views" out of v1. This design covers the first step of Phase
1.6 ("Smart Issue Selection", see `ROADMAP.md`): a view-switcher menu that the plugin shows
before loading anything, so it stops being hard-wired to a single view. It is the foundation
TF-578 ("Project Issues" view) and TF-579 ("Team Issues" view) plug into — this design does
not implement either of those views' data fetching itself.

## Scope

**In scope:**
- A menu screen shown immediately on plugin start, before any network call: **My Issues**,
  **Project Issues**, **Team Issues**.
- Only **My Issues** is selectable/functional. The other two are visible but disabled with
  a "not available yet" hint — TF-578/579 will flip them on.
- Menu navigation: ↑/↓ moves the selection (clamped, no wraparound — consistent with the
  existing issue-list navigation), Enter selects an available entry.
- Back navigation: `Esc` from inside a view (loaded list or error state) returns to the
  menu. `q` quits the app from anywhere (menu or view) — unchanged as the app-wide quit key.
- Internal restructuring of `src/plugin/app.rs`: today's `AppState` (Loading/Loaded/Error)
  is renamed to `ViewState` and nested under a new `Screen` enum (`Menu` / `View`).

**Explicitly out of scope** (tracked as separate issues, so this plan doesn't silently
expand):
- Fetching Project Issues or Team Issues data (TF-578, TF-579) — their menu entries stay
  disabled until those issues land.
- CWD → Linear-project detection (TF-577) — not needed yet since Project Issues isn't
  selectable.
- In-list search/filter (TF-580), issue creation (TF-581).
- Remembering the last-selected menu entry across app runs, or across an `Esc` back-to-menu
  (menu always resets to entry 0) — not worth the state for a 3-item menu.

## Architecture

`src/plugin/app.rs` gains a menu layer above the existing data-loading state machine,
instead of flattening menu state into one enum alongside Loading/Loaded/Error. This keeps
menu navigation and data-loading concerns independently understandable and testable, and
means TF-578/TF-579 only need to add a `fetch_*` function and flip an `available` flag —
not touch the state machine shape again.

- **`ViewKind`** — new enum: `MyIssues`, `ProjectIssues`, `TeamIssues`. Carries a `label()`
  helper for menu rendering.
- **`ViewState`** — rename of today's `AppState`. Variants unchanged: `Loading`,
  `Loaded { issues, selected }`, `Error { message }`.
- **`Screen`** — new top-level enum: `Menu { selected: usize }` | `View(ViewKind, ViewState)`.
- **`App`** — holds `screen: Screen` instead of `state: AppState`. `App::new()` now starts
  at `Screen::Menu { selected: 0 }` (previously started at `AppState::Loading`).
- **Menu options** — a small const array of `(ViewKind, &str, available: bool)`:
  `MyIssues` → available, `ProjectIssues`/`TeamIssues` → not available. This is the single
  place TF-578/TF-579 change to activate their view.

## Components

- **`src/plugin/app.rs`** — `ViewKind`, `ViewState` (renamed `AppState`), `Screen`, `App`,
  and `handle_key`. `handle_key` gains menu-mode handling (↑/↓/Enter over the const options
  array) and the new `Esc` → back-to-menu transition from `Screen::View`; `q` continues to
  map to `Action::Quit` from both `Screen::Menu` and `Screen::View`.
- **`src/plugin/ui.rs`** — `draw()` matches on `Screen` first. `Screen::Menu` renders a
  simple list: available entries in normal style, unavailable entries dimmed with a
  "(coming soon)" suffix. `Screen::View(_, view_state)` renders exactly what `draw()` does
  today for `Loading`/`Loaded`/`Error` — that match arm's body is unchanged, just reached
  one level deeper.
- **`src/main.rs`** — `load_issues`/`ensure_loaded` now write their result into
  `Screen::View(view, ...)` rather than the old flat `AppState`. The dispatched fetch call
  is still only ever `fetch_my_issues`, via `match view { ViewKind::MyIssues => ...,
  ViewKind::ProjectIssues | ViewKind::TeamIssues => unreachable!() }` — unreachable because
  the menu never lets those be selected yet.

## Data flow

1. App starts → `Screen::Menu { selected: 0 }` is drawn immediately. No network call happens
   until a view is entered (unlike today, where the first frame is already `Loading`).
2. ↑/↓ moves `selected` among the 3 menu entries, clamped at both ends.
3. Enter on the selected entry: if `available`, transitions to
   `Screen::View(view, ViewState::Loading)`, which triggers `ensure_loaded`/`fetch_my_issues`
   exactly as today. If not available, no state transition (see Error handling).
4. Inside `Screen::View`, behavior is unchanged from today: list navigation, `o` opens the
   selected issue in the browser, `r` retries on error.
5. New: `Esc` inside `Screen::View` (whether `Loaded` or `Error`) → back to
   `Screen::Menu { selected: 0 }`. `q` quits the app directly, from either screen.

## Error handling

- Selecting a disabled menu entry is a no-op — no state transition. The "(coming soon)"
  label is a static caption on the entry itself, not a transient message, so no new
  transient-message concept is needed for something TF-578/579 will remove soon anyway.
- All existing `ViewState::Error` handling (typed `LinearClient` errors, retry via `r`) is
  unchanged — it's simply reached one level deeper, under `Screen::View`.

## Testing

- **`app.rs`**: existing state-transition tests are updated to wrap expectations in
  `Screen::View(ViewKind::MyIssues, ...)` — behavior itself is unchanged. New tests: menu
  navigation clamps at both ends; Enter on the available entry transitions to
  `Screen::View(MyIssues, Loading)`; Enter on a disabled entry is a no-op; `Esc` from a
  loaded view and from an error view both return to `Screen::Menu { selected: 0 }`; `q`
  quits from both the menu and a view.
- **`ui.rs`**: new render test asserting the menu screen shows all three labels, that the
  two disabled entries carry the "(coming soon)" marker, and that "My Issues" does not.
- **`main.rs`**: `dispatch_launch_decision` is untouched. `ensure_loaded`/`load_issues` have
  no dedicated tests today (only exercised via the binary) — that stays as is.

## Open items for the implementation plan

- Exact wording/styling of the "not available yet" hint — cosmetic, decide during
  implementation.
- Whether the pane title (`ui.rs`, currently always "My Issues"/"Linear") should reflect
  the active `ViewKind` — nice-to-have, not blocking for this issue.
