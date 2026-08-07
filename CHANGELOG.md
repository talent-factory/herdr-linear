# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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
  freshly created, explicitly targeted, pre-labeled tab with exactly one pane (TF-579)

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

### Phase 1.5 - Stability
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
- Report bugs in Linear: https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b
- Ask on GitHub: https://github.com/talent-factory/herdr-linear/discussions
- Check documentation: https://github.com/talent-factory/herdr-linear#readme
