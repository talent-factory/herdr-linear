//! Benchmarks `LinearClient::execute_batch`'s throughput against a mocked
//! backend, at a range of concurrency levels.
//!
//! A fixed-size batch of requests is run through `execute_batch` at each
//! concurrency level, so the numbers show how wall-clock time drops as
//! more requests are allowed in flight at once — see `benches/README.md`
//! for how to read the output.

#[path = "support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use tokio::runtime::Runtime;

/// Requests per batch. Large enough that concurrency differences are
/// visible, small enough to keep each sample fast.
const REQUEST_COUNT: usize = 50;

fn bench_execute_batch_concurrency(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime for bench harness");
    let (client, _server) = rt.block_on(support::viewer_client());

    let mut group = c.benchmark_group("execute_batch_concurrency");

    for &concurrency in &[1usize, 5, 10, 25, 50] {
        group.bench_with_input(
            BenchmarkId::new("concurrency", concurrency),
            &concurrency,
            |b, &concurrency| {
                b.to_async(&rt).iter(|| async {
                    let requests = (0..REQUEST_COUNT).map(|_| client.get_viewer()).collect();
                    let results = client.execute_batch(requests, Some(concurrency)).await;
                    // `Iterator::all` is vacuously true on an empty
                    // collection, so also pin down the count directly —
                    // otherwise a regression that silently dropped requests
                    // instead of erroring on them could pass both checks.
                    assert_eq!(
                        results.len(),
                        REQUEST_COUNT,
                        "execute_batch should return one result per request"
                    );
                    assert!(
                        results.iter().all(|r| r.is_ok()),
                        "every mocked batch request should succeed"
                    );
                });
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = support::bench_config();
    targets = bench_execute_batch_concurrency
}
criterion_main!(benches);
