//! Benchmarks the overhead the rate-limit-retry machinery in
//! `LinearClient::execute_graphql` adds to the common case: a request that
//! succeeds on the first try and never actually retries.
//!
//! The retry-triggered path itself (`RATE_LIMIT_MAX_ATTEMPTS` further
//! attempts, each preceded by a real `tokio::time::sleep` for the backoff
//! duration) is dominated by that sleep and isn't meaningfully
//! benchmarkable — a criterion sample of it would just measure how
//! precisely the OS scheduler wakes up a sleeping task. What's worth a
//! regression baseline is the cost paid on *every* request regardless of
//! whether rate limiting ever happens: the attempt-counting loop and the
//! `Result` pattern match against `Error::RateLimitExceeded` in
//! `execute_graphql`.
//!
//! Both variants below hit the exact same success-path branch in
//! `execute_graphql` — `rate_limit_retry` is only ever consulted once a
//! `RateLimitExceeded` error has already been returned, so on success it's
//! never read. The two benchmarks are expected to be statistically
//! indistinguishable; they're kept side by side as a standing check that a
//! future refactor doesn't accidentally make the success path branch on
//! that flag and add measurable overhead to one of them.

#[path = "support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, Criterion};
use tokio::runtime::Runtime;

fn bench_rate_limit_retry_success_path_overhead(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime for bench harness");
    let (retry_enabled_client, _enabled_server) = rt.block_on(support::viewer_client());
    let (retry_disabled_client, _disabled_server) = rt.block_on(async {
        let (client, server) = support::viewer_client().await;
        (client.with_rate_limit_retry(false), server)
    });

    let mut group = c.benchmark_group("rate_limit_retry_success_path_overhead");

    group.bench_function("retry_enabled_default", |b| {
        b.to_async(&rt).iter(|| async {
            retry_enabled_client
                .get_viewer()
                .await
                .expect("mocked viewer call should not fail")
        });
    });

    group.bench_function("retry_disabled", |b| {
        b.to_async(&rt).iter(|| async {
            retry_disabled_client
                .get_viewer()
                .await
                .expect("mocked viewer call should not fail")
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = support::bench_config();
    targets = bench_rate_limit_retry_success_path_overhead
}
criterion_main!(benches);
