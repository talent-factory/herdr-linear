# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `benches/` — a `criterion`-based benchmark suite (dev-dependency only) covering `get_all_issues`'s auto-pagination, `execute_batch`'s throughput at a few concurrency levels, and the rate-limit-retry wrapper's overhead on the common (no-retry) success path, all run against a mocked backend. Run with `cargo bench`; see `benches/README.md`. Not part of `cargo test`/CI — a local/manual tool for catching regressions before they ship (TF-623)

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
