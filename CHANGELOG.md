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

### Fixed
- My Issues no longer lists completed/canceled issues (TF-582)
- Implement-on-`<Enter>`: no longer silently falls back to `.` when the working
  directory can't be determined — aborts with an actionable status instead, and the
  README now documents that `<Enter>` needs the panel opened via the split action
  (not the tab action), since only a split pane inherits the invoking pane's cwd
  (TF-584)
- Implement-on-`<Enter>`: a `q` pressed while the flow is blocking is now honored
  (quits) instead of being silently discarded along with buffered input (TF-584)
- `herdr_cli`'s response parsing now treats a top-level `{"error": ...}` body as a
  failure even on a zero exit code, matching its own documented contract (TF-584)

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
