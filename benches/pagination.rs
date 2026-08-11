//! Benchmarks `LinearClient::get_all_issues`'s auto-pagination overhead
//! against a mocked backend, at a range of page counts.
//!
//! Each sample measures the full N-page traversal (N sequential mocked
//! GraphQL round-trips, one issue per page) so the numbers scale roughly
//! linearly with page count once fixed per-call overhead is subtracted —
//! see `benches/README.md` for how to read the output.

#[path = "support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use herdr_linear::PaginationOptions;
use tokio::runtime::Runtime;

fn bench_get_all_issues_pagination(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime for bench harness");
    let mut group = c.benchmark_group("get_all_issues_pagination");

    // Page counts small enough to run in a reasonable wall-clock time, but
    // wide enough to show how per-page overhead scales.
    for &pages in &[1usize, 5, 20, 50] {
        let (client, _server) = rt.block_on(support::paginated_issue_client(pages));

        group.bench_with_input(BenchmarkId::new("pages", pages), &client, |b, client| {
            b.to_async(&rt).iter(|| async {
                let issues = client
                    .get_all_issues(None, PaginationOptions::default())
                    .await
                    .expect("mocked pagination should not fail");
                // A pagination regression that stops traversal early (e.g. a
                // `hasNextPage` off-by-one) would still return `Ok` here —
                // just with fewer issues and fewer HTTP round trips than
                // `pages`, reporting a misleadingly *faster* benchmark
                // instead of failing. Guard against that silently passing.
                assert_eq!(
                    issues.len(),
                    pages,
                    "pagination should traverse all {pages} mocked pages"
                );
                issues
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = support::bench_config();
    targets = bench_get_all_issues_pagination
}
criterion_main!(benches);
