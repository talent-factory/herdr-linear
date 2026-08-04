# Roadmap - Herdr Linear

## Vision

A complete, production-ready Rust client for Linear.app's GraphQL API, providing:
- Full API coverage
- Excellent error handling and resilience
- Integration with Herdr ecosystem
- Webhook support for real-time updates

## Phases

### Phase 1: Core Foundations ✅ (Current)

**Status**: In Progress

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
- [ ] Comprehensive test coverage (→ Phase 1.5)

### Phase 1.5: Polish & Stability

**Estimated**: August-September 2026

- [ ] Unit test coverage (80%+)
- [ ] Integration tests with Linear sandbox
- [ ] Performance benchmarks
- [ ] Rate limiting strategies
- [ ] Connection pooling for batch operations
- [ ] Better pagination helpers
- [ ] User feedback incorporation

### Phase 2: Advanced Features

**Estimated**: September-October 2026

- [ ] **Webhooks Support**
  - Real-time issue update notifications
  - Comment subscriptions
  - Project change events
  
- [ ] **Batch Operations**
  - Bulk issue updates
  - Efficient multi-issue queries
  - Transaction support

- [ ] **Filtering & Search**
  - Advanced query builder
  - Saved filters
  - Full-text search integration

- [ ] **Performance**
  - Request caching layer
  - Connection reuse
  - Parallel query execution

### Phase 3: Herdr Integration

**Estimated**: October-November 2026

- [ ] **Herdr Plugin Development**
  - Plugin SDK integration
  - Issue sync strategies
  - Team synchronization

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
2. **Limited Batch Operations**: Some operations are sequential
3. **No Offline Mode**: Requires live connection
4. **Pagination Manual**: No automatic pagination helpers

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

| Date       | Milestone                    | Status   |
|-----------|------------------------------|----------|
| Aug 2026  | Phase 1 Complete            | Current  |
| Sep 2026  | Phase 1.5 (Stability)       | Planned  |
| Oct 2026  | Phase 2 (Advanced Features) | Planned  |
| Nov 2026  | Phase 3 (Herdr Integration) | Planned  |
| Dec 2026  | Phase 4 (Production)        | Planned  |

## Feedback

- **Bugs**: Report in Linear or GitHub Issues
- **Features**: Open a discussion or PR
- **Questions**: Ask on GitHub Discussions

---

Last updated: 2026-08-04
