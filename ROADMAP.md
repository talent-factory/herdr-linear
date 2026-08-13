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
- [x] Test coverage (55 unit/integration tests across client + plugin)

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

### Plugin Polish (ongoing, not Linear-tracked)

Small UX fixes and additions that came directly from live user testing
rather than a planned milestone — landed alongside Phase 2a rather than
deferred to a dedicated phase.

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

### Phase 2: Advanced Features

**Estimated**: September-October 2026

- [ ] **Webhooks Support**
  - Real-time issue update notifications
  - Comment subscriptions
  - Project change events

- [ ] **Batch Operations**
  - Efficient multi-issue queries (beyond the implement flow, which now runs
    through `execute_batch` — see Phase 2a/TF-622)
  - Transaction support

- [ ] **Filtering & Search** — first slice in progress, see Phase 2a above
  - Saved filters
  - Full-text search integration

- [ ] **Performance**
  - Request caching layer
  - Connection reuse
  - Parallel query execution

### Phase 3: Herdr Integration

**Estimated**: October-November 2026

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

**Estimated**: November-December 2026

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
| Oct 2026  | Phase 2 (Advanced Features)     | Planned   |
| Nov 2026  | Phase 3 (Herdr Integration)     | Planned   |
| Dec 2026  | Phase 4 (Production)            | Planned   |

## Feedback

- **Bugs**: Report in Linear or GitHub Issues
- **Features**: Open a discussion or PR
- **Questions**: Ask on GitHub Discussions

---

Last updated: 2026-08-13
