# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-08-21

Phase 2a (query DSL / server-side filtering) and Phase 2b (comments, named
filter presets, implement-flow reliability hardening) in full — the first
release since 0.2.1 (2026-08-12).

### ⚠️ Breaking changes (semver, `plugin` feature)

Three changes to the `plugin` feature's public API accumulated across this
release without being versioned individually; bundled here as one migration
note rather than three separate diffs to reconcile on upgrade:

- **TF-616**: `assignee_open_filter`/`project_open_filter`/`team_open_filter`
  (`src/plugin/data.rs`) and the `fetch_*` functions built on them now
  require an additional `filter_terms: &[FilterTerm]` parameter. Pass `&[]`
  to reproduce the exact pre-0.3.0 filter behavior.
- **TF-647**: `plugin::app::Action` gained a `CyclePreset` variant. Any
  exhaustive `match` over `Action` needs a new arm (or a wildcard).
- **TF-648**: `plugin::app::Action` gained an `AddComment { issue, body }`
  variant, and `ViewState::Loaded` gained a `comment: CommentState` field.
  Same exhaustive-match caveat as above; any code constructing
  `ViewState::Loaded` directly needs the new field.

See this file's TF-616/TF-647/TF-648 entries below for the full detail on
each.

### Added

- Named filter presets: `config.toml` now supports multiple `[[filter_presets]]` entries
  (each a `name` + a `query` in the exact same DSL `default_query` already uses) alongside
  the existing single `default_query`, and a new `p` key cycles a loaded view through them
  — `default_query` (no bracket shown) → preset 0 → preset 1 → … → the last preset → back
  to `default_query` → preset 0 again, applying whichever is active via the exact same
  mechanism `default_query`/the `/`-filter already use (server-side filter terms,
  client-side `sort:`), refetching on every switch just like leaving and re-entering the
  view after editing `default_query` would. The active preset's name is shown in the list title next to the
  view name (`My Issues [Urgent]`), the same way an active `/`-filter's query text already
  is; presets are independent of that live `/`-filter, which still layers on top of
  whichever is active. `App::cycle_active_preset`/`active_preset`/`set_active_preset`
  (`plugin::app`) hold the cycling state (a new `ActivePreset { index, name }`, parallel to
  `status` rather than nested in `ViewState::Loaded`, so `set_issues`'s ~90 existing call
  sites didn't need to change); `plugin::config::resolve_filter_presets`/
  `load_filter_presets` resolve the configured list (empty when none configured — TF-647's
  AC: `p` is then a no-op and behavior is unchanged); `main.rs::resolved_query_for` (the
  new query-resolution entry point in front of the unchanged `resolved_default_query_for`
  fallback) re-reads `config.toml` fresh on every fetch, same as `default_query`, and
  degrades a malformed config, an unrecognized term in a preset's query, or a preset
  removed from the config since it was activated to a status banner rather than crashing.
  `handle_key`'s `p` binding stays pure/I/O-free like every other key — it returns a new
  `Action::CyclePreset` and leaves the actual `config.toml` read + cycle + refetch to
  `main.rs`'s `event_loop` (whose shared "draw `Loading`, await the fetch, drain a
  buffered quit if it ran long" tail is now a reusable `draw_and_load` helper, also used by
  `Action::Retry`/`Action::EnterView`). Settings (`s`) now also lists configured presets.
  ⚠️ Breaking (semver, `plugin` feature): adds a `CyclePreset` variant to the `pub`
  `Action` enum — a breaking change for any downstream consumer of that feature (matching
  the non-exhaustive-less `Action` in general). `ResolvedConfigSummary` also grows a
  `filter_presets` field, but that struct is `pub(crate)` — not externally visible — so
  the addition itself isn't breaking; see the correction on TF-617's line below, which
  made the same claim about the same struct (TF-647)
- `src/plugin/query.rs` — a hand-rolled parser for the plugin's query DSL: `priority:`/`state:`/`label:` filter terms (with `=`/`>=`/`<=` comparisons and named priority levels) and `sort:field,...` sort keys (with `-` for descending), plus a stable multi-key `sort_issues` helper. Double-quoted values (`state:"In Review"`) support multi-word names. Parsing never errors — unrecognized or malformed terms fall back to free text for the existing substring matcher (TF-580), with recognized-but-malformed terms additionally recorded in `ParsedQuery::rejected` for a future caller to surface as a hint. Not yet wired into the running plugin — server-side filter application is TF-616, `default_query`/`/`-filter integration is TF-617 (TF-615)
- `default_query` in `config.toml`: a query-DSL string (same grammar as TF-615/616) applied automatically on every view load. Filter terms narrow the fetch server-side via TF-616's `IssueFilter` merge; `sort:` terms order the fetched issues client-side via a new `main.rs::apply_fetched_issues` (shared by every view's load-issues arm). The `/`-filter is now DSL-aware too, via new `plugin::query::matches_filter_term`/`compare_issues`: a query with no recognized `key:value` tokens still takes the exact pre-existing substring-match path, but one that does narrows the already-loaded, already-`default_query`-filtered/sorted issue list client-side. A `/`-filter composes with `default_query` rather than replacing it — it can only narrow further within whatever `default_query` already fetched, and inherits `default_query`'s sort order unless the typed query has its own `sort:` — see README's "query DSL" section for the full user-facing semantics, including the caveat that `state:` can never match a terminal (`Done`/`Cancelled`) issue, since every view's base filter excludes those from the fetch entirely. Repeated same-kind filter terms (two `state:` terms, two same-comparator `priority:` terms) are now deduped by the parser itself (`push_filter_term`/`filter_terms_collide` in `query.rs`) before they reach either consumer, so the server-side merge and the client-side `/`-filter can no longer disagree on a colliding repeat; `state:`/`label:` matching is Unicode-aware (`str::to_lowercase`, not `eq_ignore_ascii_case`) to stay consistent with Linear's own `eqIgnoreCase`. A malformed `config.toml`, or a `default_query` with unrecognized DSL terms, is now surfaced as a status banner under the loaded list rather than silently applying no filter/sort; an unrecognized term typed into a `/`-filter is shown in that filter's title bar. Settings (`s`) now shows the resolved `default_query`. (Review correction: this entry originally called the `default_query` field added to `ResolvedConfigSummary` a semver-breaking change "same as TF-616's `filter_terms` parameter addition above" — but `ResolvedConfigSummary` is `pub(crate)`, not `pub`, so it's invisible outside this crate and adding a field to it isn't actually a breaking change for any downstream consumer, unlike TF-616's `pub` function signatures.) (TF-617)
- `j`/`k` now scroll the Detail pane's content, which previously had no way to reveal anything past the bottom of a long issue description — only the list pane's `↑`/`↓` scrolled (via `ratatui::List`'s own viewport tracking). `App::detail_scroll` (per-view state) is clamped in `App::detail_scroll_down` against a new `ui::detail_line_count` — the same "clamp the stored offset in `App`, estimate the real wrapped row count in `ui.rs`" split TF-585's help overlay already established, reusing its `word_wrapped_row_count` estimator against a Detail-pane-specific conservative width. Resets to `0` whenever the selected issue changes (arrow-key navigation, or a filter narrowing the list) so a new issue's description never opens mid-scroll
- Mouse wheel support, matching `herdr-file-viewer`'s own "keyboard-first, mouse additive" design: `main.rs::run_tui` now requests `EnableMouseCapture` on startup (herdr forwards mouse events to a pane that requests it) and a new `plugin::app::handle_mouse` dispatches the wheel — scrolling the List (moves `selected`, one issue per notch) or the Detail pane (scrolls `DETAIL_WHEEL_STEP` = 3 rows per notch, via the same clamped path `j`/`k` use), whichever half of the terminal the pointer is over. The help overlay, while open, owns the wheel exactly like it already owns the keyboard, instead of letting it leak through to the hidden view underneath. Clicks and drags are a deliberate no-op for now — not requested, and `App` has no click-target/divider-drag state to act on one with
- Leftover idle tab after an implement agent finishes now closes itself: `implement_one` starts a detached background watcher (`main.rs::spawn_tab_close_when_agent_is_done`) right after its prompt lands, which runs `herdr agent wait --until done` and then `herdr tab close` (new `herdr_cli::tab_close`) — this never blocks `implement_one`'s own return, so single-implement (`<Enter>`) and parallel multi-implement (`start_implementation_many`/`execute_batch`, TF-622) both return to the caller immediately regardless of how long the agent actually runs. Fails open on any `agent_wait` timeout or error (agent still working, herdr losing track of the pane, its heuristic status detection missing the "done" transition) and on a `tab_close` failure afterwards — the tab is simply left open rather than risk closing out a still-useful or failed agent's output. If the plugin quits before the agent finishes, the detached task is dropped along with the rest of the tokio runtime — and, via a new `herdr_cli::OnAbandon` parameter on `agent_wait` (defaulting to the pre-existing behavior everywhere else), this "done" wait's underlying `herdr agent wait` subprocess is now `kill_on_drop`'d too, so it doesn't linger as an orphaned process for up to its full 24h timeout after the plugin itself has already exited; no cleanup across plugin/herdr-server restarts is attempted (TF-649)
- A new `m` key composes and sends a comment on the selected issue without leaving the terminal — `client.add_comment()` was already fully implemented and tested in the library but never wired into the plugin. Bound and captured exactly like the existing `/`-filter text input (`App::is_commenting`/`start_commenting`/`push_comment_char`/`pop_comment_char`/`confirm_comment`/`cancel_comment`, `plugin/app.rs`): `m` opens editing on whichever issue is currently selected (a no-op with no selected issue — an empty list, or a filter matching nothing), `Enter` confirms, `Esc` cancels and discards the draft, and while editing every character key is captured into the draft instead of dispatching its usual view binding (`q`, `o`, `c`, `?`, ...) — the same input-mode precedence `handle_key` already gives `/`-filtering. Unlike filtering, neither `Up`/`Down` nor the mouse wheel move the selection while commenting, and `CommentState::Editing` additionally captures its target issue once when `m` is pressed rather than re-resolving "whichever issue is selected" at confirm time — two independent defenses against a still-open draft ever posting to a different issue than the one shown when composition began. `CommentState` is a small state machine (`Idle`/`Editing { issue, body }`/`Failed { issue, body }`) rather than an independent flag-plus-buffer, so `{not editing, but a leftover body}` is unrepresentable rather than merely avoided by convention; a confirmed comment that fails to send (network/API error, or, in principle, no client yet) is reopened as a resumable `Failed` draft (`App::restore_failed_comment_draft`, logged via `tracing::warn!`) instead of discarded — pressing `m` again on the same issue resumes the typed text rather than requiring it to be retyped from memory. The live draft renders in the status-banner area (`Comment on <identifier> (Enter to send, Esc to cancel): <draft>▏`, styled distinctly in cyan), grown well past the short-status-message `STATUS_BANNER_MAX_HEIGHT` cap (up to roughly half the terminal height, with a scroll fallback beyond that) so a longer draft's live cursor doesn't render off-screen, and taking precedence over any stale `status` left over from an earlier action — except a `Failed` draft, which deliberately stays out of the banner so the send failure's `Status::Error` stays visible instead of an immediately-reopened draft masking it. An empty (or whitespace-only) draft is not sent on `Enter` — editing simply stays open rather than discarding whatever was typed, the same way `Esc` is the explicit way to abandon a draft. Confirming dispatches a new `Action::AddComment { issue, body }`, handled in `main.rs`'s `event_loop` by a new `start_add_comment` (mirroring `start_implementation`'s status-banner pattern: an interim `Status::Ok` while the request is in flight, then `Status::Ok`/`Status::Error` — via `Display`, matching `load_issues`'/`ensure_loaded`'s existing `err.to_string()` convention — once it resolves). Comment history/preview in the Detail pane, Markdown preview while composing, and editing/deleting one's own comments are explicitly out of scope for this change. ⚠️ Breaking (semver, `plugin` feature): adds an `AddComment` variant to the `pub` `Action` enum and a `comment: CommentState` field to `ViewState::Loaded`, both breaking changes for any downstream consumer of that feature, matching this project's existing precedent for `Action`/`ViewState::Loaded` additions. (Review correction: an earlier revision of this entry described `CommentState` as an `editing: bool`/`body: String` struct and `Action::AddComment` as a positional `(Issue, String)` tuple — both were reworked during review, per the paragraph above, before this change ever shipped) (TF-648)

### Fixed

- `implement_one` now confirms herdr actually recognizes a real, *stable* coding agent in the pane
  before trusting `agent_wait`'s "idle" status enough to start the implement-prompt-send dance.
  Live-reproduced (not just from the bug report's screenshots): the exact stuck pane from the
  report was still sitting there, `herdr agent get` on it returning `agent_not_found`, its screen
  showing the implement prompt spliced mid-way into `hr`'s (`headroom wrap claude --memory
  --code-graph`) own plain bootstrap log — the prompt had been typed in *before* `claude` was even
  exec'd, not dropped by the target's own slower startup as TF-587/619/650 diagnosed one layer up.
  Neither `agent_wait(..., "idle", ...)`'s status nor the purely text-based `prompt_landed`/
  `wait_for_prompt_stable` check can tell a real agent's live input box apart from a wrapper's own
  non-repainting scrollback that happens to look quiet, or hold matching text, for a while — a
  slow multi-stage `agent_command` alias's own preamble can satisfy either well before the real
  agent starts. New `plugin::herdr_cli::agent_wait_for_start` polls `herdr agent get <pane_id>`
  and requires the *same* non-blank `agent` identity on `AGENT_START_CONFIRM_POLLS` (3) consecutive
  polls — not just one — before returning, mirroring `agent_wait_for_exit`'s consecutive-poll
  confirmation pattern but for the opposite transition (an agent appearing rather than
  disappearing); requiring several matching polls in a row additionally guards against herdr
  transiently misidentifying one of a multi-stage wrapper's own intermediate helper processes
  (e.g. `hr`'s rtk-hook installer) as "the agent" before the real target takes over. Runs in
  `implement_one` between the existing `pane_run` and `agent_wait(..., "idle", ...)` calls, under
  its own `AGENT_START_WAIT_TIMEOUT_MS` (20s — roughly 3x the ~6-7s real-world worst case observed
  against `hr`) budget, separate from `agent_wait`'s own 30s "idle" budget so a pane that never
  starts an agent at all fails close to the former bound rather than compounding both. `AGENT_NOT_FOUND_POLL_INTERVAL`
  (previously module-private) is now `pub`, reused as this new wait's poll interval rather than
  duplicating the same tuning constant (TF-669)
- Follow-up review pass on the above: a `herdr agent get` response that parses but carries no
  identity (a missing/blank/wrong-typed `agent.agent` field — distinct from herdr's own explicit
  `agent_not_found`) now logs the raw response via `tracing::debug!` instead of silently degrading
  into indistinguishable "not started yet" polling, so a future herdr protocol change doesn't burn
  the full timeout with no diagnostic trail; the wait's timeout error now also carries a summary of
  the *last* poll's outcome (`still agent_not_found` / an identity that never stabilized / a
  tolerated error count) instead of only restating the timeout it hit, and `implement_one`'s own
  failure message is reworded from the overstated "agent never started" to "herdr never confirmed
  the agent started" — three consecutive herdr hiccups produce this failure too, not only a pane
  that genuinely never got an agent running in it. The read-only `agent get` poll itself now uses
  `OnAbandon::KillChild` (matching `agent_wait_for_exit`'s identical poll) instead of
  `LeaveRunning`, so an abandoned poll doesn't orphan a child process. Single-issue implement now
  redraws mid-flight during this wait too (previously only the later prompt-send phase did, per
  the next entry below) via a new `ImplementProgress` enum folding both waits' progress signals
  into the one callback `start_implementation` can safely give `&mut app`/`&mut terminal` to.
  Added direct unit tests for `agent_identity` (blank/missing/wrong-typed/null cases) and three
  `agent_wait_for_start` regression tests this pass's own review found missing: the
  error-tolerance-exhausted path pinned to its exact attempt count (mirroring
  `agent_wait_for_exit`'s equivalent), an intervening `agent_not_found` forcing a full confirmation
  recount, and a blank-identity response doing the same (TF-669)
- Single-issue `<Enter>` implement now shows visible progress while `send_prompt_until_visible` is
  (re)confirming the prompt landed, instead of leaving the screen looking unchanged for the whole
  wait. Diagnosed live against a real `headroom wrap claude ...`-style multi-process
  `agent_command`: `agent_wait`'s "idle" status resolves in single-digit milliseconds, long before
  the target's own input loop has attached, so the *first* (re)send is a near-certain miss rather
  than an occasional one — every single-issue implement already paid a mandatory multi-second wait
  for the second (re)send (typically the one that lands) with zero on-screen indication anything
  was still happening. That's the exact shape of "prompt not injected, but a manual second
  `<Enter>` usually works": the user is very plausibly looking at this same silent,
  already-in-progress wait and giving up on it early. `send_prompt_until_visible`/`_with` now take an
  `on_attempt(attempt, attempts)` progress hook, called once per (re)send; `implement_one` forwards
  it, and the new `PromptSendPolicy` struct bundles the four retry-tuning parameters that hook
  would otherwise have pushed past clippy's `too_many_arguments` threshold. `start_implementation`
  (single-issue only) wires it to redraw the status via the new pure `prompt_attempt_status`
  helper, skipping attempt 1 (redundant with the "Starting implementation for X…" status already
  on screen) and showing "…confirming attempt N of M…" from attempt 2 on. The concurrent
  multi-issue path (`implement_many`, TF-622) passes a no-op — several of its futures can be
  mid-flight at once and none of them owns the one shared terminal safely, so it's unchanged. This
  does not change the resend/confirmation logic or its timing budgets, only what's visible while
  it runs (TF-650)
- TF-649's leftover-tab auto-close now tells the plugin when it actually closes a tab, instead of
  doing so silently. Two tabs vanished mid-implement with the agent still visibly working, before
  the user had said `/exit`; with no on-screen signal to tell "finished" from "lost", one of the
  two was assumed dead and re-run — on already-completed work. `close_tab_once_agent_is_done` (and
  `spawn_tab_close_when_agent_is_done`, its `tokio::spawn` wrapper) now take the issue identifier
  plus an `mpsc::UnboundedSender<plugin::app::Status>`; on a successful `tab_close` only, it sends
  the new pure `tab_auto_closed_status(identifier)` — `"{identifier}: agent finished, tab
  closed."` — through it. `event_loop` owns the matching receiver and drains it, via the new pure
  `drain_notifications` helper (non-blocking `try_recv`, so it can never stall the render/input
  loop), once per tick, before `terminal.draw`, turning any pending notice into a `Status` banner
  the same way every other action already reports through `App::set_status`. Both
  `Action::Implement` and `Action::ImplementMany` now draw once more immediately after their
  blocking implement call returns, before looping back to that per-tick drain — otherwise a
  notice queued during that call could silently overwrite a same-tick outcome status (including a
  `Status::Error`) before it had ever been rendered, which is exactly the class of invisible-
  outcome bug this ticket exists to fix, just inverted. Neither existing fail-open branch
  (`agent_wait` timing out, `tab_close` itself failing) sends anything — a notice on either would
  falsely claim "finished" for a tab that's still open. The channel is threaded through
  `implement_one`/`implement_many`/`start_implementation`/`start_implementation_many`, all of
  which gain a required parameter; all four are private functions in the `herdr-linear` binary
  target, so this has no public API impact — the parameter-threading pattern itself matches this
  codebase's established precedent for a new cross-cutting parameter through the implement-flow
  call chain (see TF-650's `on_attempt` above). Investigated and explicitly dropped from this
  ticket's scope: distinguishing a clean exit from a crash before auto-closing — verified via
  `herdr api schema --json` that herdr exposes no exit code/signal anywhere in its socket API
  (`AgentStatus` is a 5-value screen-content heuristic, `PaneInfo`/the `pane_exited` event carry
  neither), so there is currently no reliable, non-heuristic way to make that distinction; tracked
  separately as TF-654, blocked on herdr (TF-653)
- Review follow-up on the entry above: the notify-drain clobber it fixed for `Action::Implement`/
  `Action::ImplementMany` (an explicit `terminal.draw()` immediately after each blocking call
  returns) was itself both too narrow and, in the strict sense, still racy. Too narrow because
  every other `event_loop` arm that sets a status without `.await`ing anything first — `Action::
  OpenInBrowser`'s clipboard-copy failure, both of `Action::OpenConfig`'s branches (its post-editor
  one worst of all: the external-editor hand-off blocks for however long the user is in their
  editor, a far wider window for a notice to land in than either implement call), `Action::
  CyclePreset`'s error branch — had exactly the same unprotected shape and simply hadn't been
  named in the original bug report. Still racy because "drawn once" isn't "seen": two
  `terminal.draw()` calls with no real wait between them (an outcome status's draw, immediately
  followed by the next tick's drain-then-redraw once `flush_buffered_quit`'s non-blocking poll
  returns) can both reach the terminal within the same tick, with no guarantee an emulator ever
  painted the first before the second arrived. `event_loop` now tracks whether the *previous*
  `crossterm::event::poll` returned because an event arrived, and skips that tick's
  `drain_notifications` when it did — deferring the drain until a `poll` genuinely times out, i.e.
  until the terminal has been sitting on the current draw, undisturbed, for a real dwell time (up
  to the full 200ms `poll` window) — reusing the exact mechanism that already gives every other
  status banner its dwell time, rather than inventing a separate minimum-display-duration timer.
  This protects every action arm uniformly, so the two explicit post-`.await` draws this ticket
  originally added are gone; nothing arm-specific replaces them. `drain_notifications` additionally
  logs (`tracing::debug!`) any notice it discards before ever drawing it — the "only the last
  survives" tradeoff itself is unchanged (an accepted class of overwrite, same as every other
  `set_status` call), but with `implement_many`'s concurrency it's plausible for more than one tab
  to auto-close within a single poll window, and a dropped notice from *this* class shouldn't also
  be unrecoverable from a log file, since a tab silently vanishing with no signal is the exact bug
  TF-653 exists to prevent. The loop-level ordering this relies on is still not covered by an
  automated test — `event_loop` remains hardcoded to a real `CrosstermBackend` terminal and can't
  be driven by `TestBackend` — reasoned about via the (now single, non-duplicated) doc comment on
  `event_loop`'s `skip_drain_this_tick` instead, same as before (TF-653)
- TF-649's leftover-tab auto-close was watching the wrong signal: it closed the tab (and sent the
  TF-653 "agent finished" notice) once herdr's `agent_status` reported `"done"`, but live use
  against a real Linear issue showed it firing the moment the initial implement prompt finished,
  while opening a PR, getting it reviewed, and fixing findings were still entirely manual steps
  meant to happen in that same pane. Root cause, per herdr's own skill doc: `"done"` isn't a
  completion signal at all, it's "the same underlying idle state [as `idle`] after unseen
  background work finishes" — a tab-focus heuristic that has no relationship to whether the issue
  itself is actually done. `close_tab_once_agent_is_done`/`spawn_tab_close_when_agent_is_done` are
  renamed to `close_tab_once_agent_has_exited`/`spawn_tab_close_when_agent_has_exited` and now
  wait on the new `plugin::herdr_cli::agent_wait_for_exit` instead of `agent_wait(..., "done",
  ...)`: it polls `herdr agent get <pane_id>` (a new, deliberately coarse
  `AGENT_EXIT_POLL_INTERVAL`, 10s — this wait can run for the same 24h budget the old one did) and
  only succeeds once herdr's `agent_not_found` error code is reported `AGENT_EXIT_CONFIRM_POLLS`
  (3) times in a row, which typically happens once the user types `/exit`, when every manual step
  for that issue is genuinely finished, not just when the coding agent goes idle after the first
  prompt — requiring a consecutive run rather than trusting a single poll keeps a lone herdr-side
  identity-tracking blip from being mistaken for the agent actually being gone. A lone non-
  `agent_not_found` poll error (a `DEFAULT_CLI_TIMEOUT` expiry under load, a momentary herdr
  daemon hiccup) is likewise tolerated up to `AGENT_EXIT_POLL_ERROR_TOLERANCE` (3) times in a row
  before giving up, since this poll-based wait can make thousands of individual calls over its
  24h budget where the old single-subprocess `agent_wait` call only made one. Both fail-open paths
  (a `tab_close` failure, or the exit poll timing out/erroring after its tolerance is exhausted)
  are unchanged from TF-649/TF-653: the tab is left exactly as it is, and no notice is sent
  (TF-668)
- `HERDR_LINEAR_LOG_FILE`-based logging (`main.rs::init_tracing`) now actually emits `debug!`-level
  records — every `tracing::debug!` call this crate has ever shipped (`send_prompt_until_visible_with`'s
  attempt-failure logging, `flush_buffered_quit`'s discarded-key count, and the mid-flight
  redraw-failure log added above) was silently dropped before reaching the log file, even with the
  env var set and pointing at a writable path: `tracing_subscriber::fmt()` with no explicit level
  defaults to `INFO`, and `init_tracing` never overrode it (unlike `examples/tracing_demo.rs`'s own
  `EnvFilter`-based setup, which this function doesn't share). Caught while verifying the
  mid-flight-redraw fix above actually left a trace — it didn't, for the same reason. Now sets
  `.with_max_level(tracing::Level::DEBUG)`; opting into this diagnostic mode at all (setting the
  env var) is already the signal that debug-level detail is wanted (TF-650)

### Changed

- `c` (open `config.toml`) no longer opens a separate herdr tab for the editor. It used to create a fresh pane and type `nvim '<path>'` into its shell (`herdr pane run`) — after quitting `nvim`, that pane's shell was left behind, requiring a manual close. Now, mirroring `herdr-file-viewer`'s own editor hand-off, herdr-linear's own TUI steps aside in-place (leaves raw mode/the alternate screen, drops mouse capture) and runs the editor as a direct child process taking over the same pane; quitting the editor returns straight to the plugin's own screen, with nothing left to close. `main.rs`'s `open_config_in_herdr_pane`/`HerdrPaneError`/`editor_tab_is_alive` and the now-unused `herdr_cli::tab_list`/`tab_focus`/`pane_list`/`pane_process_info`/`find_existing_editor_tab`/`find_root_pane_for_tab`/`is_pane_alive` are all removed — this specific flow no longer talks to the herdr CLI at all. `editor.rs`'s `EDITOR_AGENT_NAME`/`build_editor_command` are removed too (no more herdr pane to label, no more shell-quoted command to type); the `editor`-resolution logic itself (`config.toml` override, else `nvim` on `PATH`, else the OS opener) is unchanged. `suspend_tui`/`resume_tui` attempt every step regardless of an earlier one's failure (mirroring `run_tui`'s own teardown discipline), and a new `EditorOutcome::TerminalNotRestored` means a failed terminal restore is now always surfaced as a status error instead of the previous behavior — a resume failure after a successful edit silently cleared the status bar with no indication anything went wrong. `run_editor_in_terminal_with` is the new pure, injectable-closure core this composition runs through, unit-tested independently of the real terminal/subprocess
- ⚠️ Breaking (semver, `plugin` feature): `assignee_open_filter`/`project_open_filter`/`team_open_filter` (`src/plugin/data.rs`) now accept a `&[FilterTerm]` (TF-615's parsed `priority:`/`state:`/`label:` terms) and deep-merge each into the base `IssueFilter` JSON server-side, alongside the existing open/not-completed/not-canceled constraint — e.g. a `state:` term merges its `name` comparator into the same `"state"` key the base filter's `type: { nin: [...] }` already occupies, rather than replacing it. `state:`/`label:` terms match by name case-insensitively (`eqIgnoreCase`); `priority:` terms with different comparators (`priority:>=2 priority:<=4`) combine into one range, while two terms landing on the same JSON key (two `state:` terms, two same-comparator `priority:` terms) resolve last-wins, with the earlier one silently dropped. `fetch_my_issues`/`fetch_project_issues`/`fetch_current_project_issues`/`fetch_team_issues`/`fetch_current_team_issues` now thread a `filter_terms` slice through to the same effect — only `fetch_my_issues`/`fetch_current_project_issues`/`fetch_current_team_issues` are called directly from `main.rs`; `fetch_project_issues`/`fetch_team_issues` are called internally by the `fetch_current_*` variants. Every current call site passes `&[]`, which is a documented no-op reproducing the exact pre-TF-616 filter JSON — `default_query`/`/`-filter integration (TF-617) is what will start passing real terms. Adds a required parameter to seven `pub` functions behind the `plugin` feature, which is a breaking change for any downstream consumer of that feature (TF-616)

## [0.2.1] - 2026-08-12

### Added

- `benches/` — a `criterion`-based benchmark suite (dev-dependency only) covering `get_all_issues`'s auto-pagination, `execute_batch`'s throughput at a few concurrency levels, and the rate-limit-retry wrapper's overhead on the common (no-retry) success path, all run against a mocked backend. Run with `cargo bench`; see `benches/README.md`. Not part of `cargo test`/CI — a local/manual tool for catching regressions before they ship (TF-623)

### Changed

- Implement-on-`<Enter>`: each issue's per-issue agent name (e.g. `hr--tf-579`) is now applied
  by a best-effort `agent rename` call *after* the agent starts, rather than being passed at
  launch. Nothing passes a name at launch under herdr >= 0.8.0 (see TF-624 below), so the
  0.2.0 auto-retry on herdr's `agent_name_taken` error has been removed — there is no longer a
  launch-time name collision for it to recover from. A failed rename is now reported as a
  warning and the agent keeps running under herdr's own default name (TF-624)

### Fixed

- `c` (open `config.toml`) and Implement-on-Enter both silently failed against herdr >= 0.8.0,
  which redesigned `agent start`/`agent wait`/`agent send` out from under this plugin: `agent
  start` dropped `--cwd`/`--tab`/`--focus` + arbitrary argv in favor of `--kind`/`--pane` against
  a fixed enum of recognized agent binaries (unable to launch `nvim` or a custom `agent_command`
  wrapper alias like `"hr"`), `agent wait` renamed `--status` to `--until`, and `agent send` was
  replaced by `agent prompt`. Both flows now open their tab via `tab_create` (unchanged) and type
  the launch command into its root pane via a new `pane_run` wrapper instead — herdr's own
  passive auto-detection picks up whatever recognized agent ends up running, same as before.
  TF-604's "upgrade herdr" hint (below) was addressing a different, no-longer-applicable case;
  see TF-624 for the actual current-herdr incompatibility and its fix (TF-624)

- TF-604's `--cwd`-rejection hint assumed the *only* way an installed herdr could reject `--cwd`
  on `agent start`/`tab create` was predating `min_herdr_version = 0.7.0`. That's no longer true
  for `agent start`: herdr >= 0.8.0 (well above the floor) rejects it too, having redesigned the
  subcommand's flags entirely (see TF-624) — the hint's wording is now only accurate for
  `tab_create`, the one remaining `--cwd`-accepting call (TF-624)

- `min_herdr_version` (in `herdr-plugin.toml`, mirrored by `MIN_HERDR_VERSION` in
  `herdr_cli.rs`) raised `0.7.0` → `0.8.0`: the new `pane_run`/`tab_list`/`tab_focus`/
  `agent_rename`/`agent_prompt`/`agent_wait --until` calls this fix introduces have only ever
  been verified against herdr 0.8.0 — publishing the old, now-inaccurate `0.7.0` floor would
  send users on an older herdr into the exact silent-failure this ticket exists to fix. See the
  new "Requirements" section in `README.md` (TF-624)

- Implement-on-`<Enter>`: when the installed `herdr` CLI is older than the version that added
  `--cwd` support to `agent start`/`tab create`, the raw "unknown option: --cwd" herdr reports is
  now followed by a hint that herdr-linear requires herdr >= 0.7.0 and needs upgrading, instead of
  leaving the user to guess why a tab was created but the agent never started (TF-604)

## [0.2.0] - 2026-08-11

### Added

- Auto-paginating `LinearClient` helpers — `get_all_issues`, `get_all_teams`, `get_all_team_issues`, `get_all_projects` — that loop through every page of a query and return the full result set, with a configurable page size and safety caps on total pages/items (TF-609)
- `LinearClient` now automatically retries requests that fail with `Error::RateLimitExceeded`: it waits the server's `Retry-After` value (falling back to exponential backoff, capped at 60s, when Linear doesn't send a usable one), retries up to 3 total attempts, and still surfaces the original `RateLimitExceeded` error unchanged if the budget is exhausted. Rate limiting is detected both via Linear's documented HTTP 400 + `RATELIMITED` GraphQL error code and via a plain HTTP 429 (kept as a defense-in-depth fallback). Opt out via `LinearClient::with_rate_limit_retry(false)` to keep the old fail-fast behavior (TF-610)
- `c` (open `config.toml`) now opens `nvim` inside a herdr pane by default — usable over SSH, where the previous OS-default-opener behavior wasn't. Set `editor` in `config.toml` to use a different editor instead; if neither resolves or the herdr pane can't be opened, `c` falls back to the OS's default opener as before. Repeated `c` presses reuse the same editor pane (TF-614)

### Fixed

- `c` (open `config.toml`) now works from any screen and view state — Menu, a view still
  loading, a loaded view, and the Error screen alike — instead of only firing after actually
  hitting an error. The Keybindings help overlay's `c` entry moved from "Error screen" to
  "Global" to match (TF-614)
- Implement flow: the prompt-landed confirmation now polls the pane continuously until the
  sent prompt has been visible, with no gaps, for a documented stability window — instead of
  checking at exactly two fixed offsets (500ms, then 800ms later) and declaring success from
  those two samples alone. A live repro against a slow-starting target showed the prompt land,
  pass both of those samples, and then still get wiped by the target's own async startup
  finishing after that 1.3s window had already elapsed, reporting success on an agent left with
  an empty prompt box (TF-619)
- Retry/EnterView action arm: a `q`/Ctrl+C pressed while `ensure_loaded()` is blocking is
  now drained and honored once the fetch returns, matching the Implement/ImplementMany arms
  — but only once the fetch has actually taken long enough (past 1s) to be plausibly stuck.
  TF-610's rate-limit retry can hold this arm for up to ~2 minutes with the screen looking
  frozen and no visible way to quit; a normal fast round-trip still lets a buffered key fall
  through to the loop's next poll cycle instead of being silently discarded (TF-610)
- Herdr host context: `focused_pane_cwd`/`workspace_cwd`/`cwd` values with stray leading or
  trailing whitespace are now trimmed before use, instead of surviving untrimmed into git's
  `current_dir` (repo auto-detection) and the herdr CLI's `--cwd` argument
  (implement-on-`<Enter>`), where either could break
- Detail pane: unordered Markdown list items now render with a `•` bullet and a hanging
  indent for wrapped continuation lines, so a wrapped line starting with `--` (e.g. inline
  code like `` `cargo test --features plugin -- --ignored live_api` `` wrapping right
  before `--ignored`) can no longer be mistaken for a new bullet. Ordered (`1. `) list
  items keep their numbering but get the same hanging indent on wrap (TF-613)

### Removed

- Unused `graphql_client`, `async-trait`, `anyhow`, `dotenvy`, and `tokio-test` dependencies
  — none were referenced anywhere in the crate. `reqwest` upgraded from the legacy 0.11 line
  to 0.12, collapsing the dependency tree to a single hyper 1.x stack instead of duplicating
  hyper 0.14/http 0.2 alongside it

## [0.1.1] - 2026-08-10

### Added

- Cross-platform release pipeline: checksum-verified prebuilt binaries for macOS/Linux/Windows via tag-triggered GitHub Actions, replacing always-compile-from-source installs
- Full Windows platform support in herdr-plugin.toml, with dedicated PowerShell action launchers working around a herdr pane-spawn limitation
- In-app Help overlay (`?` key): What's New / Keybindings / Settings / About (TF-585)
- Type-to-filter the loaded issue list by title/identifier (TF-580)
- Guaranteed tab-per-issue on the Linear implement flow (TF-579)
- Unique per-issue herdr agent names + multi-select issues (TF-590)

- Initial project setup
- Core `LinearClient` implementation
- GraphQL query/mutation execution
- Viewer (authenticated user) queries
- Teams management queries
- Issues queries and mutations
- Comments management
- Projects and cycles support
- Workflow states queries
- Comprehensive error handling
- Logging with tracing
- Examples for basic usage and issue operations
- CI/CD pipeline with GitHub Actions
- Documentation and README
- Contributing guide
- Roadmap
- Herdr plugin layer, gated behind the new `plugin` Cargo feature: a ratatui/crossterm
  TUI panel showing the viewer's open assigned Linear issues (navigate, open in browser,
  retry on error), API key resolution from the plugin config file or `LINEAR_API_KEY`,
  the `herdr-plugin.toml` manifest, and the `scripts/open-split.sh` / `scripts/open-tab.sh`
  idempotent launcher scripts
- Herdr plugin view switcher: menu-first interface allowing users to choose between
  My Issues and Project Issues (both implemented) and Team Issues (not yet available)
- Implement-on-`<Enter>`: pressing Enter on a selected issue opens a herdr tab, starts
  the preferred coding agent, sets the issue to "In Progress" via a real GraphQL
  mutation, and injects an implement prompt once the agent is ready; configurable
  `agent_command` fallback in `config.toml` (TF-584)
- `c` keybinding on any Linear error screen (no project matches, multiple projects match,
  etc.) opens `config.toml` with your OS's default handler for `.toml` files (creating the
  file/directory first if either is missing), instead of requiring you to quit the plugin
  and find the path yourself (TF-588)
- `/` keybinding on a loaded view's issue list opens type-to-filter: narrows the list live
  by title or identifier (case-insensitive substring match), `↑`/`↓` still navigate the
  narrowed list, `<Enter>` confirms and keeps the filter applied, `Esc` cancels and restores
  the full list (TF-580)
- Multi-select in the issue list: `<Space>` marks/unmarks the selected issue (shown with a
  `[x]`/`[ ]` checkbox prefix), and `<Enter>` with one or more issues marked implements all
  of them sequentially, summarizing the results in one status banner (e.g. "3/4 started",
  plus a message per issue that failed or finished with a warning); unmarked `<Enter>`
  behaves exactly as before. Marking is independent of the active filter — marks target the
  underlying issue, not its position in a narrowed list, so they survive a filter change
  (TF-590)

### Changed
- The Linear project override in `config.toml` is now a `[project_overrides]` table keyed
  by repo name instead of a single flat `project_id` value. The flat key was scoped to the
  plugin *installation*, not the repo — since one `config.toml` is shared by every
  repo/workspace using the plugin, setting it for one repo silently redirected every other
  repo sharing that install to the same project too. An old `project_id = "..."` entry is
  now simply ignored (falls back to name matching) rather than erroring. Never
  released/documented, so no migration is needed (TF-588, found while fixing TF-589)
- The "no project matches"/"multiple projects match" error messages now show the resolved
  `config.toml` path and a ready-to-paste `[project_overrides]` snippet for the current
  repo, instead of a generic "Set `project_id` in config.toml to override" (TF-588)

### Fixed
- My Issues no longer lists completed/canceled issues (TF-582)
- Implement-on-`<Enter>` and Project Issues detection now resolve the working directory from
  herdr's injected `HERDR_PLUGIN_CONTEXT_JSON` launch context instead of the plugin process's
  own `std::env::current_dir()` (always the plugin's own install directory), so both now work
  correctly whether the panel was opened via the split action or the tab action — previously
  only a split pane happened to inherit the right cwd. If the working directory still can't be
  determined either way, implement-on-`<Enter>` now sets an actionable status instead of
  silently starting the agent with an empty `--cwd` (TF-577, TF-584)
- Implement-on-`<Enter>`: a `q` pressed while the flow is blocking is now honored
  (quits) instead of being silently discarded along with buffered input (TF-584)
- `herdr_cli`'s response parsing now treats a top-level `{"error": ...}` body as a
  failure even on a zero exit code, matching its own documented contract (TF-584)
- Implement-on-`<Enter>`: `resolve_agent_command` now prefers an explicit `agent_command`
  over the agent derived from other open herdr tabs (was the other way around). herdr's
  tab list can only report the underlying binary a pane runs, never the alias/wrapper used
  to launch it, so a pane started via an `hr`-style alias was indistinguishable from one
  started bare — under the old precedence, `agent_command` (including the `"hr"` default)
  could never actually take effect once any other Claude Code tab was open (TF-584)
- Implement-on-`<Enter>`: `agent_wait` now retries (bounded, budget-aware) when `herdr agent
  wait` returns a response missing the `result` field — a reproducible herdr v0.7.3 bug where
  its wait stream closes as soon as the pane's agent identity is detected, well before the
  agent is actually idle. Previously this surfaced as an immediate "agent didn't become
  ready" error and the implement prompt was never injected (TF-584)
- Implement-on-`<Enter>`: a status banner reported after `agent_wait`/`agent_send` fails no
  longer discards warnings collected earlier in the same flow (e.g. a failed tab/pane setup step or a
  failed "In Progress" transition) — every terminal status now includes every warning, not
  just the one on the path that happened to finish last (TF-584)
- `herdr` CLI calls other than `agent_wait` (`agent_list`, `tab_create`, `agent_start`, `pane_close`,
  `agent_send`) are now individually timeout-bounded, so a hung `herdr` daemon can no longer
  freeze the whole panel indefinitely (TF-584)
- `agent_wait`'s missing-`result`-field retry is now detected via a dedicated error variant
  instead of matching a substring of the formatted error message, and its retry budget can no
  longer be silently overrun by one extra attempt once the caller's timeout is exhausted
  (TF-584)
- `is_valid_agent_command` now also rejects glob metacharacters (`* ? [ ] ~`) and `!` (bash
  history expansion, live since the command runs through `sh -i`) (TF-584)
- Implement-on-`<Enter>`: starting a second issue while an earlier issue's agent tab is
  still running under the same `agent_command` no longer fails with a raw
  `agent_name_taken` internal error. Each issue's `herdr agent start` call now uses a name
  unique to that issue (the resolved command plus the issue identifier, e.g. `hr--tf-579`)
  instead of reusing the bare command string for every issue, and if herdr still reports
  the name as taken, the call retries automatically with one of herdr's suggested
  candidates before giving up (TF-590)
- Implementing two Linear issues back to back could leave both agents sharing one
  mislabeled tab: `agent_start` never told herdr where to place the new agent pane, so it
  inherited herdr's implicit default placement (often a split into whichever tab currently had
  focus), and a follow-up tab rename would then relabel whatever tab that turned out to be —
  possibly a different, already-running issue's tab. Every implemented issue now gets a
  freshly created, explicitly targeted, pre-labeled tab, with its now-redundant extra pane
  closed on a best-effort basis (a failure to close it is a non-fatal warning, not an abort)
  (TF-579)

### Removed
- Unused `cli` Cargo feature (and its `clap` dependency), superseded by the `plugin` feature

## [0.1.0] - 2026-08-04

### Added
- First public release
- Full GraphQL API support for:
  - User queries (viewer)
  - Team queries and filtering
  - Issue queries, creation, and updates
  - Comment management
  - Project queries
  - Cycle queries
  - Workflow state queries
- Comprehensive error types
- Async/await support with tokio
- Structured logging with tracing
- Unit tests
- Integration test examples
- Complete documentation

---

## Version Guidelines

### When to bump versions:

**MAJOR (X.0.0)**: Breaking API changes
- Removing or significantly altering public methods
- Changing error type hierarchy
- Modifying core behavior

**MINOR (0.X.0)**: New features, backwards compatible
- Adding new query methods
- Adding new model types
- Extending existing types with optional fields
- Improving performance

**PATCH (0.0.X)**: Bug fixes, documentation
- Fixing incorrect behavior
- Improving error messages
- Documentation updates
- Internal refactoring

### Release Process

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md` with changes
3. Create git tag: `git tag -a vX.X.X -m "Release vX.X.X"`
4. Push tag: `git push --tags`
5. Publish to crates.io: `cargo publish`

---

## Unreleased Features (Planned)

See [ROADMAP.md](ROADMAP.md) for planned features and timeline.

### Phase 1.7 - Stability
- [ ] Improved test coverage
- [ ] Integration tests
- [ ] Performance benchmarks

### Phase 2 - Advanced Features
- [ ] Webhook support
- [ ] Batch operations
- [ ] Advanced filtering
- [ ] Caching layer

### Phase 3 - Herdr Integration
- [ ] Plugin SDK integration
- [ ] Bidirectional sync
- [ ] Custom workflows

### Phase 4 - Production
- [ ] Security audit
- [ ] Official publication on crates.io
- [ ] Production deployment guide

---

## Support

For issues or questions about versions:
- Report bugs / request features: https://github.com/talent-factory/herdr-linear/issues
- Ask questions: https://github.com/talent-factory/herdr-linear/discussions
- Check documentation: https://github.com/talent-factory/herdr-linear#readme
