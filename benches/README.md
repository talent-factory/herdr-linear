# Benchmarks

Performance baselines for `LinearClient`'s three hot code paths, run against
a mocked GraphQL backend (`mockito`), so a change can be compared against a
previous run instead of guessing.

| Benchmark | File | What it measures |
| --- | --- | --- |
| `get_all_issues_pagination` | `pagination.rs` | Auto-pagination overhead of `get_all_issues`, at 1/5/20/50 mocked pages |
| `execute_batch_concurrency` | `batch_execution.rs` | `execute_batch` throughput for a fixed 50-request batch, at concurrency 1/5/10/25/50 |
| `rate_limit_retry_success_path_overhead` | `rate_limit_retry.rs` | The retry-wrapper's cost on the common case — a request that succeeds on the first try and never retries |

The rate-limit-retry path itself (an actual retry, i.e. further attempts
each preceded by a real `Retry-After`/backoff `sleep`) is deliberately
**not** benchmarked: its wall-clock cost is dominated by that sleep, so a
criterion sample of it would mostly measure OS scheduler wake-up jitter, not
anything `herdr-linear` controls. What's worth a baseline is the overhead
paid on *every* request regardless of whether rate limiting ever happens —
see the doc comment at the top of `rate_limit_retry.rs`.

## Running

```bash
cargo bench
```

Or a single suite:

```bash
cargo bench --bench pagination
cargo bench --bench batch_execution
cargo bench --bench rate_limit_retry
```

criterion writes an HTML report (per-benchmark plots plus an index) to
`target/criterion/report/index.html` if `gnuplot` is installed; otherwise it
falls back to its own `plotters`-based charts in the same location. Either
way, running `cargo bench` again compares the new run against the previous
one and reports a percentage change with a significance estimate — that's
the actual regression signal to watch for, not the raw µs/ms numbers, which
vary by machine.

## Reading the numbers

Each benchmark reports a `[low estimate, point estimate, high estimate]`
confidence interval, e.g.:

```text
get_all_issues_pagination/pages/20
                        time:   [3.3343 ms 3.3824 ms 3.4262 ms]
```

- Treat the **point estimate** (middle value) as the number to compare
  across runs; the interval is criterion's confidence bound, not noise to
  read into.
- Pagination scales roughly linearly with page count — that's the auto-pagination
  loop's per-page overhead (one more mocked round trip, one more
  `Vec` extend). A super-linear jump between page counts would flag a
  regression in the pagination bookkeeping itself, not just added network
  cost.
- `execute_batch`'s time should *drop* as concurrency rises (more requests
  in flight at once) and then flatten out once concurrency exceeds what the
  mocked backend can usefully parallelize. A concurrency level that's
  *slower* than a lower one is the signal to look at the `buffered()`
  scheduler for a regression.
- The two `rate_limit_retry_success_path_overhead` variants
  (`retry_enabled_default` vs. `retry_disabled`) are expected to be
  statistically indistinguishable — see the doc comment in
  `rate_limit_retry.rs` for why. If a future change makes them diverge,
  that's the regression this benchmark exists to catch.

## Why these aren't part of `cargo test` / CI

`cargo test`, and CI's `cargo test --all-features --verbose` job, never
build or run `[[bench]]` targets on their own — only `cargo bench` (or
`cargo test`/`clippy` invoked with `--benches`/`--all-targets`) does. So
these benchmarks stay off the merge-gate path by default; `clippy
--all-targets` (also run in CI) still lints them, so they're held to the
same code-quality bar as everything else, just not executed. This is a
local/manual regression-catching tool, not something that should ever block
a PR — a single machine's numbers aren't comparable across CI runners
anyway, and the point is to run before/after a change on one machine.

## Why a smaller-than-default criterion profile

Every operation benchmarked here goes through a real localhost TCP
connection to a `mockito` mock server, and `mockito` does not keep those
connections alive — each round trip opens a brand-new client-side socket,
which then sits in `TIME_WAIT` for tens of seconds. A single mocked round
trip costs on the order of 100µs, so criterion's *default* measurement
window (5s measurement + 3s warm-up, per benchmark input) would fire tens
of thousands of fresh connections per input. Across this suite's ~11
benchmark inputs that's enough to exhaust the local ephemeral port range
before a plain `cargo bench` finishes — reproduced reliably on macOS during
development (`AddrNotAvailable`, "Can't assign requested address").

`benches/support/mod.rs`'s `bench_config()` trades statistical precision
(a 10-sample, ~70ms-per-input window instead of criterion's 100-sample, 8s
default) for finishing reliably out of the box, on a laptop or CI runner,
without needing OS-level socket tuning. That's still enough samples to
catch a real regression — this suite's actual goal — just not enough
sustained connection load to run a loopback host out of ports.
