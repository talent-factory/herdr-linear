//! Shared mock-backend helpers for the `herdr-linear` benchmark suite.
//!
//! Each `benches/*.rs` binary builds its own `mockito` server (a real,
//! localhost-bound HTTP server) once per benchmark input, *before* handing
//! control to criterion's timing loop, so the only work actually being
//! measured is `LinearClient`'s own code — request construction, response
//! deserialization, pagination bookkeeping, and (for `execute_batch`) the
//! `buffered()` concurrency scheduler — not mock setup.
//!
//! The request/response JSON shapes mirror the unit tests in
//! `src/client.rs` (`sample_issue_json`/`issues_page`), duplicated here
//! rather than shared because benchmark targets compile as separate crates
//! linking against `herdr_linear`'s public API and can't reach into the
//! library crate's private `#[cfg(test)]` helpers.
//!
//! This file lives at `benches/support/mod.rs` rather than
//! `benches/support.rs` so cargo's bench auto-discovery (which only picks
//! up files directly under `benches/`) doesn't treat it as its own
//! benchmark target.
//!
//! Each `[[bench]]` target is its own compiled binary and pulls this file
//! in separately via `#[path = "support/mod.rs"] mod support;`, so no
//! single bench uses every helper here — `dead_code` is allowed crate-wide
//! for that reason, same as a shared `tests/common/mod.rs`.
#![allow(dead_code)]

use criterion::Criterion;
use herdr_linear::LinearClient;
use serde_json::{json, Value};
use std::time::Duration;

/// A criterion profile intentionally smaller than the crate's defaults.
///
/// Every operation benchmarked in this suite goes through a real localhost
/// TCP connection to a `mockito` mock server, and (confirmed empirically —
/// see the PR for TF-623) `mockito` does not keep those connections alive:
/// every request/response round trip opens a brand-new client-side socket,
/// which then sits in `TIME_WAIT` for tens of seconds. A single mocked
/// round trip costs on the order of 100µs, so criterion's *default*
/// measurement window (5s measurement + 3s warm-up, per benchmark input)
/// would fire tens of thousands of fresh connections per input. Across this
/// suite's ~11 benchmark inputs that's enough to exhaust the local
/// ephemeral port range before a plain `cargo bench` finishes — this was
/// reproduced reliably on macOS during development (`AddrNotAvailable`,
/// i.e. "Can't assign requested address").
///
/// This profile trades statistical precision for finishing reliably, out
/// of the box, on a laptop or CI runner: still enough samples to catch a
/// real regression (which is this suite's actual goal — see
/// `benches/README.md`), not enough sustained connection load to run a
/// loopback host out of ports.
pub fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(20))
        .measurement_time(Duration::from_millis(50))
        .sample_size(10)
}

/// Builds a single mocked `Issue` JSON node matching the shape `QUERY_ISSUES`
/// selects (see `src/queries.rs`).
pub fn sample_issue_json(id: &str, identifier: &str) -> Value {
    json!({
        "id": id,
        "identifier": identifier,
        "title": "Bench issue",
        "description": null,
        "url": format!("https://linear.app/team/issue/{identifier}"),
        "priority": 0,
        "estimate": null,
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-01T00:00:00Z",
        "startedAt": null,
        "completedAt": null,
        "state": {
            "id": "state-1",
            "name": "Todo",
            "type": "unstarted"
        },
        "team": {
            "id": "team-1",
            "key": "ENG",
            "name": "Engineering",
            "description": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        },
        "assignee": null,
        "creator": null,
        "cycle": null,
        "project": null,
        "labels": { "nodes": [] }
    })
}

/// Wraps issue `nodes` in the `{ data: { issues: { nodes, pageInfo } } }`
/// envelope `get_issues`/`get_all_issues` expect.
pub fn issues_page(nodes: Value, has_next_page: bool, end_cursor: Option<&str>) -> String {
    json!({
        "data": {
            "issues": {
                "nodes": nodes,
                "pageInfo": {
                    "hasNextPage": has_next_page,
                    "hasPreviousPage": false,
                    "startCursor": null,
                    "endCursor": end_cursor
                }
            }
        },
        "errors": null
    })
    .to_string()
}

/// A successful `{ data: { viewer: ... } }` response body for `get_viewer`.
fn viewer_response_body() -> String {
    json!({
        "data": {
            "viewer": {
                "id": "user-1",
                "email": "bench@example.com",
                "name": "Bench User",
                "avatarUrl": null,
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-01T00:00:00Z"
            }
        },
        "errors": null
    })
    .to_string()
}

/// Spins up a mock server that answers every `get_viewer()` call with the
/// same successful response, and a client wired to it.
///
/// The returned `ServerGuard` must be kept alive for as long as `client` is
/// used — dropping it tears the mock server down.
pub async fn viewer_client() -> (LinearClient, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/graphql")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(viewer_response_body())
        .create_async()
        .await;

    let client = LinearClient::with_endpoint("lin_api_bench", format!("{}/graphql", server.url()))
        .expect("a well-formed dummy API key should construct a client");

    (client, server)
}

/// Spins up a mock server that answers `get_all_issues`'s auto-pagination
/// with exactly `pages` pages (one issue per page), and a client wired to
/// it.
///
/// Each page's mock is keyed to the `after` cursor `get_all_issues` sends
/// for that page, so the full N-page traversal replays identically on every
/// criterion iteration. The returned `ServerGuard` must be kept alive for
/// as long as `client` is used.
pub async fn paginated_issue_client(pages: usize) -> (LinearClient, mockito::ServerGuard) {
    assert!(
        pages > 0,
        "paginated_issue_client requires at least one page"
    );

    let mut server = mockito::Server::new_async().await;
    let mut after: Option<String> = None;

    for page in 0..pages {
        let after_matcher = match &after {
            None => r#""after":null"#.to_string(),
            Some(cursor) => format!(r#""after":"{cursor}""#),
        };
        let has_next_page = page + 1 < pages;
        let end_cursor = has_next_page.then(|| format!("bench-cursor-{page}"));

        server
            .mock("POST", "/graphql")
            .match_body(mockito::Matcher::Regex(after_matcher))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(issues_page(
                json!([sample_issue_json(
                    &format!("issue-{page}"),
                    &format!("ENG-{page}")
                )]),
                has_next_page,
                end_cursor.as_deref(),
            ))
            .create_async()
            .await;

        after = end_cursor;
    }

    let client = LinearClient::with_endpoint("lin_api_bench", format!("{}/graphql", server.url()))
        .expect("a well-formed dummy API key should construct a client");

    (client, server)
}
