# App performance and review consistency — 2026-11-04

This change finishes the measured parts of draft #17 on top of the latest Openings work
(`#16`, baseline `239bdd8`). It retains review settings, classification rules, analysis version,
stored reviews, saved reports, access controls and both supported distributions.

## Workload and results

Measurements used a generated archive made from committed fixtures: 1 account, 1,100 games and
60,500 moves, with synthetic scores. No live database was opened or copied. Both release binaries
ran sequentially on the same macOS arm64 machine (12 logical CPUs), with background workers and
network sync disabled. Each invocation made a disposable SQLite backup and verified the source
file stayed unchanged. Cold here means the first request to a fresh server, not a cold OS cache.

Three alternating before/after runs each made one cold request and 15 warm requests per endpoint.
The table reports the median of the three cold measurements and the median of the three warm
medians, in milliseconds. Small differences in sub-millisecond endpoints are noise.

| Page API | Cold before | Cold after | Warm before | Warm after |
|---|---:|---:|---:|---:|
| Session | 0.198 | 0.201 | 0.122 | 0.147 |
| Status | 0.352 | 0.279 | 0.208 | 0.183 |
| Activity events | 0.163 | 0.138 | 0.136 | 0.132 |
| Accounts | 0.430 | 0.412 | 0.223 | 0.216 |
| Review queue | 0.211 | 0.206 | 0.164 | 0.165 |
| Games list | 1.938 | 1.863 | 1.597 | 1.604 |
| Game review | 1.130 | 1.166 | 0.927 | 0.959 |
| Opening filters | 0.741 | 0.694 | 0.633 | 0.628 |
| Account insights | 115.410 | 71.528 | 0.525 | 0.531 |
| Practice | 51.137 | 50.719 | 48.286 | 49.211 |
| Openings, White | 6.206 | 6.028 | 4.180 | 4.155 |
| Openings, Black | 7.537 | 7.365 | 7.185 | 6.999 |
| Reports | 0.253 | 0.222 | 0.159 | 0.152 |
| Settings | 0.145 | 0.165 | 0.140 | 0.140 |

Cold Insights improved by **38%** by retaining only the tag presence and hanging-piece name
needed for aggregation, instead of allocating complete explanation trees for every move. The
parser preserves malformed-data handling, duplicate-key behavior, nesting limits and numeric
validation. Full insight outputs still match the preserved Python oracle. All 14 API payload
hashes matched across all six benchmark runs after excluding timestamps and operational
`data_revision`/database-size fields. Practice remains the slowest uncached page query in this
workload; this PR does not claim a backend speedup there.

Chromium measured actual encoded response bodies using a fresh browser context for each page,
including its document, loaded scripts, styles and fonts; API bodies and HTTP headers are excluded.
Prebuilt Brotli/gzip reduced first-load static transfers by **63–64%** across all 13 page types.
Overview fell from 565,838 to 203,965 bytes; Insights from 582,697 to 208,170 bytes; Openings from
549,959 to 197,629 bytes. This is a transfer-size measurement, not a promised page-load-time gain.

The UI now uses the existing database revision to refresh changed results. Unchanged Insights
stop polling, stale cache results still refresh, and transient failures retry. Board move maps and
shapes are stable across unrelated renders. Openings refreshes after reviews finish, including
fast re-reviews whose game counts never change. Practice keeps active puzzles stable during these
updates and ignores delayed responses from a skipped puzzle or a different account.

Raw per-run cold/warm/p95 timings, fingerprints and every page's transfer sizes are in
[app-speed.json](performance-results/app-speed.json).

## Concurrent review replay

A separate real Stockfish check used four generated fixture games (141 plies), two workers with
one thread and 64 MiB hash each, depth 16, MultiPV 2, a 5-second cap and depth-10 threat probes.
Cold review performed 225 searches and shared one concurrent search; warm review performed zero
searches. Every stored review field matched after excluding duration, timestamps and revision/ID
metadata. Source bytes remained unchanged. The 26.065-second cold / 0.151-second warm timings are
diagnostics, not a speed comparison with the old implementation; other validation work also ran
on this development machine. See [review-concurrency.json](performance-results/review-concurrency.json).

Per-database in-flight coordination makes workers with the same complete cache key consume the
same result. Deterministic transport tests also cover follower cancellation, leader cancellation,
engine failure, distinct keys and separate databases. Warm replay still requires successful cache
persistence and retained entries; separate application processes do not share this coordinator.

Review engine scheduling priority remains unchanged. Since search limits include wall-clock time,
lower priority under contention could reduce achieved depth even with the same configured budget.

## Reproduce safely

Run from the repository root, after installing development dependencies and building Rust/UI:

```sh
.venv/bin/python scripts/seed_benchmark.py benchmark-output/app-fixture.db --scale 100
.venv/bin/python scripts/benchmark_api.py --source benchmark-output/app-fixture.db \
  --out benchmark-output/app-api.json --repeat 15 --browser
.venv/bin/python scripts/seed_benchmark.py benchmark-output/review-fixture.db
cargo run --release --locked --manifest-path rust/Cargo.toml --example review_benchmark -- \
  --source benchmark-output/review-fixture.db --games 4 --workers 2 \
  --out benchmark-output/review-replay.json
```

Set `PLYBACK_STOCKFISH` to the verified engine executable for the real-engine check. The API tool's
`--binary` and `--frontend` flags select a separately built baseline. `--browser` needs Chromium
installed through Playwright. Use new output paths on each run. Benchmarks reject SQLite sidecars,
write only disposable copies and never overwrite existing results. Run comparisons sequentially
without concurrent builds or engine tests for cleaner timing evidence.

## Validation and remaining work

Local validation covered Rust fmt, all-target Clippy, 100 release tests plus the example's snapshot
safety test; 179 non-live Python tests and Ruff; 19 installer tests; frontend lint, 18 unit tests,
TypeScript/production build and 22 Chromium browser workflows. Rust, Python and npm dependency
audits found no known vulnerabilities. Six additional WebKit checks passed with iPhone 13 emulation,
and the Practice/Openings phone layouts were visually inspected.
The container built successfully and passed health, authentication, CSRF, compressed-asset equality
and error-cache checks with a disposable volume, read-only filesystem and non-root user.

Openings now supports 8/15-move depth selection. Practice supports phase filters and preserves
filtered statistics after answers. Transposition merging, continuing an accepted alternative with
new live replies, owner feedback on Openings, and physical iPhone/Android checks over Tailscale
remain follow-ups in #21/#6. Browser emulation does not close the real-device checks.
