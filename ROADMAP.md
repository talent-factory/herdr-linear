# Roadmap - Herdr Linear

## Vision

A complete, production-ready Rust client for Linear.app's GraphQL API, providing:
- Full API coverage
- Excellent error handling and resilience
- Integration with Herdr ecosystem
- Webhook support for real-time updates

## Phases

### Phase 1: Core Foundations ✅ Complete

- [x] Basic GraphQL query/mutation execution
- [x] Viewer/authentication
- [x] Teams and issues queries
- [x] Issue creation and updates
- [x] Comments management
- [x] Projects and cycles queries
- [x] Workflow states
- [x] Error handling and logging
- [x] Documentation and examples
- [x] CI/CD pipeline
- [x] Test coverage (721 unit/integration tests across client + plugin, as of
      2026-08-21 — up from 55 at Phase 1's close; see Phase 2b below for what
      grew it)

### Plugin v1: "My Issues" Panel ✅ Complete (Current)

Delivered ahead of the original schedule as part of what Phase 3 called
"Herdr Plugin Development" — a read-only Herdr split-pane/tab plugin.

- [x] Plugin manifest, launcher scripts, `herdr plugin install`
- [x] Config resolution (`config.toml` → `LINEAR_API_KEY`)
- [x] TUI: loading/loaded/error states, list navigation
- [x] Fetch the viewer's assigned issues
- [x] Open selected issue in browser, retry-on-error

**Known gap**: the panel only ever shows *my* assigned issues across *all*
teams/projects — no way to scope to the project you're actually working in,
or to switch views. See Phase 1.6 below.

### Phase 1.6: Smart Issue Selection (Plugin v2) ✅ Complete

Closed the gap between this Rust reimplementation and the original
[JacquesvanWyk/herdr-linear](https://github.com/JacquesvanWyk/herdr-linear)
plugin it's modeled after, which lets you drill into projects and search
rather than only ever showing "my issues". Tracked as issues in the
[herdr-linear Linear project](https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview)
(MVP milestone, 100% — TF-576 through TF-590).

- [x] View switcher: My Issues / Project Issues / Team Issues (TF-576)
- [x] Detect the Linear project for the current working directory (git
      repo → Linear project mapping) and show its open issues (TF-577, TF-578)
- [x] Issue search/filter within the panel (fzf-style) (TF-580)

### Phase 1.7: Polish & Stability ✅ Complete

Closed out ahead of the original September estimate. Tracked as issues in the
[herdr-linear Linear project](https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview)
(Phase 1.7 milestone, 100% — TF-609 through TF-614, TF-619).

- [x] Auto-paginating helpers for list queries — `get_all_issues`, `get_all_teams`,
      `get_all_team_issues`, `get_all_projects` (TF-609)
- [x] Automatic rate-limit retry with backoff (TF-610)
- [x] Bounded-concurrency batch execution (`LinearClient::execute_batch`) —
      connection pooling itself already came for free via the shared
      `reqwest::Client`; what was missing was a capped-concurrency way to run
      several independent requests at once (TF-611)
- [x] Live integration tests against Linear's sandbox (TF-612)
- [x] Detail pane: distinct bullet + hanging indent for wrapped Markdown list
      items, so a wrapped line starting with `--` can't be mistaken for a new
      bullet (TF-613)
- [x] `c` (open `config.toml`) now works from every screen, with real
      editor-handling for SSH use (TF-614)
- [x] Fixed implement-prompt confirmation false-positives when agent startup
      outlasts the fixed 800ms window (TF-619)

Performance benchmarks and open-ended "user feedback incorporation" were
dropped from this phase's original scope; benchmarks move to Phase 2a below.
The one loose end this phase left — `execute_batch` (TF-611) not yet wired
into `main.rs`'s multi-issue implement flow — closed via TF-622 (Phase 2a).

### Phase 2a: Filtering, Batching & Performance Foundations ✅ Complete

**Target**: 2026-08-14. Tracked as issues in the
[herdr-linear Linear project](https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview)
(Phase 2a milestone). Closes out Phase 1.7's two loose ends, then delivers the
first slice of Phase 2's "Filtering & Search".

- [x] Wire `execute_batch` (TF-611) into `main.rs`'s multi-issue implement
      flow, replacing its still-sequential per-issue loop (TF-622)
- [x] Performance benchmarks (`criterion`) for pagination, batch execution,
      and rate-limit retry (TF-623)
- [x] Query DSL parser: filter terms (`priority:`, `state:`, `label:`) + sort
      keys (TF-615)
- [x] Wire the parsed filter terms into Linear's `IssueFilter` so filtering
      happens server-side (TF-616) — depends on TF-615
- [x] `config.toml` `default_query` + a DSL-aware `/`-filter, backward
      compatible with TF-580's plain substring match (TF-617) — depends on
      TF-615 and TF-616

TF-622/TF-623 are independent and can run in parallel with, or ahead of,
TF-615→616→617's dependency chain.

### Phase 2b: Comments, Filter Presets & Implement-Flow Reliability ✅ Complete

**2026-08-14 to 2026-08-21.** Started as informal "Plugin Polish" (small UX
fixes from live use, not yet Linear-tracked) alongside Phase 2a, but grew into
a full phase's worth of work — two real features (named presets, in-app
comments) plus a sustained hardening pass on the implement-agent flow, which
turned out to have more edge cases than Phase 1.7's TF-619 fix alone closed.
Tracked as issues in the
[herdr-linear Linear project](https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview)
(TF-585, TF-614 rework, TF-647 through TF-669). Test suite grew from 55 to
721 over this stretch.

- [x] Keyboard scrolling (`j`/`k`) for the Detail pane, previously unscrollable
      once a long issue description ran past the bottom of the panel — clamped
      against an estimated wrapped-row count, mirroring the help overlay's own
      scroll design (TF-585)
- [x] Real mouse support: wheel scroll for the List/Detail panes and the help
      overlay (`EnableMouseCapture` on startup), matching `herdr-file-viewer`'s
      "keyboard-first, mouse additive" design — clicks/drags remain a
      deliberate no-op
- [x] `c` (open `config.toml`) now hands the editor the terminal in-place
      (suspends/resumes the plugin's own TUI around a direct child process)
      instead of opening a separate herdr tab — fixes a leftover shell pane
      left behind after quitting the editor, and removes the herdr-CLI-pane
      machinery (`open_config_in_herdr_pane` and friends) that TF-614
      originally added for it
- [x] Named filter presets: `config.toml` `[[filter_presets]]` entries cycled
      with `p`, alongside the existing single `default_query` — supersedes
      Phase 2's "Saved filters" item below (TF-647)
- [x] `m` composes and sends a comment on the selected issue without leaving
      the terminal — `client.add_comment()` existed in the library but was
      never wired into the plugin (TF-648)
- [x] Leftover idle tab after an implement agent finishes now closes itself:
      right after `implement_one`'s prompt lands, a detached background
      watcher runs `herdr agent wait --until done` and then `herdr tab
      close` — fails open (tab stays put) on any timeout or error, and never
      blocks the synchronous single-/multi-implement flow it's spawned from,
      so parallel multi-implement (TF-622) still returns immediately per
      issue (TF-649)
- [x] `o` (open the selected issue's URL) now copies it to the clipboard via
      an OSC 52 escape sequence instead of always shelling out to the host's
      `open`/`xdg-open` — over SSH/Mosh (e.g. an iPad client) that used to
      open the URL on the host machine no one is looking at, where a
      terminal that passes OSC 52 through can put it straight into the
      client's own clipboard instead (TF-652)
- [x] TF-649's auto-close now surfaces a visible status banner when it
      actually closes a tab, instead of a tab silently vanishing with no
      signal; the same notify-drain clobber race this fix first patched for
      auto-close is fixed for every action that posts a background status,
      not just Implement (TF-653)
- [x] `state:` filter terms on completed/canceled states returned zero
      results — a query-DSL `state:` term was being AND-ed against the base
      fetch's own open/not-completed exclusion instead of replacing it,
      making "show me Done issues" unsatisfiable (TF-659)
- [x] Auto-close (TF-649/TF-653) was firing on herdr's "done" *heuristic*
      status — a screen-content guess — rather than the agent's actual
      process exit, so it could close a tab mid-PR/mid-review, before the
      agent ever reached `/exit`. Now waits for the real exit signal (TF-668)
- [x] The implement prompt could get typed into a multi-stage
      `agent_command` wrapper's own pre-agent bootstrap output (e.g. `hr` =
      `headroom wrap claude --memory --code-graph`, which runs several
      seconds of its own shell output before `claude` starts) instead of the
      real agent's input box. `implement_one` now confirms herdr recognizes
      the *same* stable agent identity on several consecutive polls before
      trusting the pane enough to send it anything (TF-669)

**Known gap, parked**: TF-654 documents that herdr's own API currently has no
way to distinguish a clean agent exit from a crash for the auto-close
watcher above — see Known Limitations below.

### Phase 2c: Release & Search 🚧 Planned

**Target**: 2026-08-28. Short, low-risk milestone proposed in the 2026-08-21
ROADMAP triage: `main` had drifted 20 commits / 3 documented breaking changes
behind `develop` since v0.2.1 (2026-08-12), and full-text search was the one
piece of Phase 2's original scope with a concrete, evidenced use — everything
else in Phase 2/3/4/Nice-to-Have below was deliberately *not* carried into a
milestone (see the "Phase 2 and beyond" note under Schedule & Milestones).

- [ ] Cut a 0.3.0 release: `develop` → `main`, bundling Phase 2a/2b and the
      three breaking `plugin`-feature changes (TF-616, TF-647, TF-648) into
      one versioned, tagged release instead of letting `main` drift further
      (TF-672)
- [ ] Full-text search integrated into the query DSL — the one still-open,
      concretely-scoped item carried over from Phase 2's "Filtering &
      Search" below (TF-673)
- **Watch, not a ticket**: the implement-flow reliability cluster
  (TF-587→619→649→653→668→669, six tickets over two weeks) looks stable
  after TF-669, but that history earns one more round of real-world
  confirmation before calling it closed — no new ticket unless something
  actually surfaces

### Phase 2: Advanced Features

**Estimated**: Unscheduled (see Schedule & Milestones note below)

- [ ] **Webhooks Support**
  - Real-time issue update notifications
  - Comment subscriptions
  - Project change events

- [ ] **Batch Operations**
  - Efficient multi-issue queries (beyond the implement flow, which now runs
    through `execute_batch` — see Phase 2a/TF-622)
  - Transaction support

- [ ] **Filtering & Search** — first slice delivered in Phase 2a, saved
      filters delivered in Phase 2b
  - ~~Saved filters~~ — delivered as named `[[filter_presets]]` (TF-647,
    Phase 2b)
  - Full-text search integration — planned for Phase 2c (TF-673)

- [ ] **Performance**
  - Request caching layer
  - Connection reuse
  - Parallel query execution

### Phase 3: Herdr Integration

**Estimated**: Unscheduled (see Schedule & Milestones note below)

- [x] **Herdr Plugin Development** — see Plugin v1/v2 above

- [ ] **Sync Engine**
  - Bidirectional sync with Herdr
  - Conflict resolution
  - Change tracking

- [ ] **Custom Workflows**
  - Automation rules
  - Template support
  - Custom field mapping

### Phase 4: Production & Ecosystem

**Estimated**: Unscheduled (see Schedule & Milestones note below)

- [ ] **Stability & Security**
  - Security audit
  - Performance tuning
  - Production deployment guide

- [ ] **Documentation**
  - Complete API reference
  - Advanced recipes
  - Troubleshooting guide

- [ ] **Community**
  - Example applications
  - Plugin marketplace
  - User forum support

- [ ] **Distribution**
  - Publish to crates.io
  - Package managers (Homebrew, etc.)
  - Docker images with examples

## Nice-to-Have Features

### Developer Experience

- [ ] CLI tool for direct Linear interaction
- [ ] Visual query builder
- [ ] GraphQL introspection browser
- [ ] OpenAPI spec generation

### Advanced Features

- [ ] Custom fields support
- [ ] Attachment handling (upload/download)
- [ ] Estimates and time tracking
- [ ] Dependency graph
- [ ] Multi-workspace support

### Integration

- [ ] GitHub integration helpers
- [ ] Slack notification builders
- [ ] Jira migration tools
- [ ] Zapier/IFTTT support

## Technical Debt & Maintenance

- [ ] Dependency updates schedule
- [ ] MSRV (Minimum Supported Rust Version) policy
- [ ] Breaking change management
- [ ] Deprecation strategy
- [ ] Long-term support plan

## Known Limitations

### Current (Will be addressed)

1. **No Webhook Support**: Events must be polled
2. **Limited Batch Operations**: `execute_batch` (TF-611) is wired into
   `main.rs`'s multi-issue implement flow (TF-622); bulk issue updates and
   multi-issue queries elsewhere still go one request at a time
3. **No Offline Mode**: Requires live connection
4. **Crash vs. clean exit is indistinguishable for auto-close** (TF-654):
   the leftover-tab auto-close watcher (TF-649/653/668) can't currently tell
   "the agent exited cleanly via `/exit`" from "the underlying process
   crashed" — herdr's `AgentStatus` is a screen-content heuristic
   (`idle`/`working`/`blocked`/`done`/`unknown`) with no exit-code/signal
   exposed over its socket API (verified via `herdr api schema --json`;
   the real `ExitStatus` only reaches herdr's own server log). Blocked on
   herdr's own API surface — parked until herdr exposes exit status, or a
   non-heuristic alternative shows up; not something this repo can fix on
   its own

### By Design

1. **Rust-Only**: No TypeScript or Node.js dependencies
2. **Async-First**: No blocking API
3. **GraphQL-Only**: Uses only Linear's GraphQL API

## Contributing to Roadmap

Have ideas? Please:

1. Open an issue in Linear
2. Start a discussion on GitHub
3. Submit a PR with feature implementation
4. Vote on issues with reactions

## Schedule & Milestones

| Date       | Milestone                       | Status    |
|-----------|----------------------------------|-----------|
| Aug 2026  | Phase 1 Complete                | Done      |
| Aug 2026  | Plugin v1 ("My Issues") Complete | Done      |
| Aug 2026  | Phase 1.6 (Smart Issue Selection)| Done      |
| Aug 2026  | Phase 1.7 (Polish & Stability)   | Done      |
| 2026-08-14| Phase 2a (Filtering/Batching/Perf)| Done      |
| 2026-08-21| Phase 2b (Comments/Presets/Reliability)| Done |
| 2026-08-28| Phase 2c (Release & Search, TF-672/673)| Planned |
| TBD       | Phase 2 remainder beyond 2c (Webhooks/Batch/Perf)| Unscheduled — see triage note below |
| TBD       | Phase 3 (Herdr Integration)     | Unscheduled — see triage note below |
| TBD       | Phase 4 (Production)            | Unscheduled — see triage note below |

*Note: the Oct/Nov/Dec 2026 estimates previously here were placeholders set
before any phase past 1.7 had a concrete plan. Actual delivery since then has
tracked Linear tickets (TF-6xx) end-to-end rather than these phases, and has
skewed toward the plugin/implement-flow (Phase 2b) over the originally
broader Phase 2/3/4 scope — see the 2026-08-21 triage discussion for a
proposed next milestone in place of blindly continuing down this list.*

## Feedback

- **Bugs**: Report in Linear or GitHub Issues
- **Features**: Open a discussion or PR
- **Questions**: Ask on GitHub Discussions

---

Last updated: 2026-08-21
