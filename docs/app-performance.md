# Performance and measurement

## Current behavior

The web build produces Brotli and gzip assets. The server negotiates these with the browser's
`Accept-Encoding` header; response sizes depend on which assets a page loads and what is cached.

Insights cache parsed move rows and responses per database. The database revision invalidates
changed results, and stale responses can be served while a background refresh runs. The interface
uses revision changes to refresh affected views and avoids polling unchanged Insights indefinitely.

Review workers in one process share an in-flight engine search only when the complete cache key
matches, including position history, engine and search settings. The durable position cache belongs
to the database. Separate application processes do not share in-flight searches. Warm replay depends
on retained cache entries and successful cache writes; it does not measure first-time analysis speed.

## Measure review throughput safely

The [review benchmark](../rust/examples/review_benchmark.rs) runs the real queue twice on disposable
database copies: first with an empty position cache, then with the first run's populated cache.
It compares stored summaries and moves, excluding internal IDs and measurement timestamps, and
fails if replay changes their values.

Create a consistent, separately named snapshot with the application's SQLite backup command.
Never supply a live database to the benchmark or copy only the main file of a running WAL database.
From the repository root, with the application built and Stockfish installed:

```sh
just backup benchmark-output/review-snapshot.sqlite
cargo run --release --locked --manifest-path rust/Cargo.toml -j 4 --example review_benchmark -- \
  --source benchmark-output/review-snapshot.sqlite --games 4 --workers 2 \
  --out benchmark-output/review-replay.json
```

Set `PLYBACK_STOCKFISH` to the verified engine executable when it is outside the normal data directory.
Use a new filename for each backup and each results file. The source must have no SQLite sidecars, cannot
be named `plyback.db`, and must contain at least the requested number of reviewable games (1 to 50).
Results must be written directly inside `benchmark-output/`; existing output files are refused.
Backups contain account history, password hashes and sessions, so keep the source snapshot private.

The benchmark checks that source bytes remain unchanged. It removes accounts, reports, reviews and
cache entries only from disposable copies; no account sync runs against the snapshot. It fixes depth
16, MultiPV 2, a five-second per-position cap and depth-10 threat probes. Worker count can be overridden;
thread and hash settings come from the snapshot. The JSON records these settings, platform, logical CPU
count, per-game completion times, wall time, cache counts, timing totals and result digests without PGNs
or player identities.

Run comparisons sequentially with the same source snapshot, engine and settings. Avoid concurrent
builds, engine tests and other CPU-heavy work. Record hardware and software versions and repeat runs.
Wall-clock search caps mean contention can change achieved search depth even with identical budgets;
check result equality as well as elapsed time. Report cold and warm measurements separately.

## Measure API and browser work

Use an isolated server with its own backup copy and background sync disabled when profiling page APIs.
Record the dataset size, filters, endpoint, response status, first-request latency and repeated-request
latencies. Verify response contents before comparing timings; cache hits and stale refreshes are different
workloads. Preserve the original snapshot and discard the working copy after the measurement.

For browser transfers, build the interface and use a fresh browser context for each page. Browser network
tools can distinguish encoded bytes from decoded resource size. State whether API bodies, fonts, headers
and browser cache hits are included. Transfer-size reductions alone do not establish a page-load-time gain.
