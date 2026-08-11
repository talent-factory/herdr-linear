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
//! never read.
//!
//! **What this pair of benchmarks can and can't tell you:** the thing being
//! measured (a single bool read plus a `Result` pattern match) costs
//! single-digit nanoseconds, while the mocked network round trip both
//! variants pay for costs on the order of 100µs — four to five orders of
//! magnitude larger. Confirmed empirically: `retry_enabled_default` and
//! `retry_disabled` show *non-overlapping* confidence intervals even on
//! unmodified code, because the difference between two independent
//! mocked-network samples is bigger noise than the thing being compared.
//! **Do not treat a difference between the two variants' numbers as a
//! signal** — at this magnitude it isn't one. What each variant *is* good
//! for is its own absolute number, tracked release over release: a
//! sustained, order-of-magnitude jump in either `retry_enabled_default` or
//! `retry_disabled` on its own points at a real regression in
//! `execute_graphql`'s success path (or in the mocked round trip itself);
//! the two variants diverging from *each other* does not.

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
