# Native backend and move-review migration

Issue [#1](https://github.com/klemen-r/plyback/issues/1) moves the production backend to Rust while
preserving the existing interface, public API, account history, imports, settings and reports.
The Python implementation remains a development oracle. Source and container launches use the
native executable exclusively; neither Python nor Node is required by the deployed server.

The retained oracle's development minimum is Python 3.11: its existing locked Starlette 1.10,
filelock 4.2.7, platformdirs 4.14.3 and websockets 17.4 require it. CI exercises 3.11 and 3.12;
correcting the old 3.10 claim preserves the current dependency pins.

## Why Rust

Rust gives the HTTP server, PGN parser, job ownership and engine transport explicit ownership and
bounded concurrency in one distributable executable. Axum/Tokio handles HTTP; blocking database,
password and chess work stays off the reactor. Shakmaty supplies legal moves and PGN interpretation;
SQLite retains the existing schema and appended transactional migrations. Stockfish does the expensive
searching, so changing languages alone is not claimed to make a new game search dramatically faster.

The frontend design remains open. This migration preserves A, D, A+D and C studies and changes only
methodology copy and test integration in the interface.

## Measured insight work

On one macOS arm64 machine, the committed public fixtures were analysed with a deterministic fake
engine, then copied 100 times: **1,100 games and 60,500 moves**. This measures application aggregation,
not engine quality or new-game diversity. Five repeated uncached computations follow one cold call.
Both implementations consume the same archive; the Python baseline retains its existing warm move
cache. Rust loads owned typed SQL rows for each call and does not cache move rows.

| Computation | Python | Native Rust | Time reduction |
|---|---:|---:|---:|
| Cold full insights | 424.60 ms | 177.15 ms | 58.3% |
| Full insights, median of five | 275.90 ms | 166.86 ms | 39.5% |
| Blitz/White filter, median of five | 94.80 ms | 65.06 ms | 31.4% |

Profiling an initial native implementation identified JSON allocation during move loading:
approximately 145 ms fell to 53–57 ms after direct typed-row extraction. No insight sections,
games or tactical output were dropped. Six filter combinations compare every output field against
the preserved Python oracle with a floating-point tolerance of 1e-8; repeated native output is exact.
Separate tests cover database isolation, rapid invalidation, fixed report membership and immutability.

The same fixture workload on the [GitHub Ubuntu runner](https://github.com/klemen-r/plyback/actions/runs/41528184314)
also improved, with both implementations measured sequentially on that runner:

| Computation | Python 3.12 | Native Rust release | Time reduction |
|---|---:|---:|---:|
| Cold full insights | 1,025.70 ms | 354.84 ms | 65.4% |
| Full insights, median of five | 614.80 ms | 234.90 ms | 61.8% |
| Blitz/White filter, median of five | 204.20 ms | 87.50 ms | 57.1% |

Raw measurements and CI provenance are in [performance-results](performance-results/). API response-cache
timings are excluded from these tables. These are two measured machines, not a hardware-independent
speed guarantee; compare before/after within each environment.

## Engine budget and correctness cost

The baseline real-engine capture uses Stockfish 19, one thread, 64 MiB hash, depth 16, MultiPV 2,
threat depth 10 and a 5-second cap per search, with no node cap. The two fixture games contain 88 and
45 plies. On the local machine the baseline took 20,729.75 ms and 10,266.40 ms; recorded-search replay
for both games had a 52.89 ms median. Native local cold benchmarking was stopped before producing
measurements; the intensive comparison below ran remotely on the same Ubuntu runner.

| Engine workload | Python baseline | Native Rust |
|---|---:|---:|
| Cold 88-ply game | 21,184.56 ms | 32,794.14 ms |
| Cold 45-ply game | 10,515.00 ms | 15,447.36 ms |
| Recorded-search processing, both games, median of five | 77.53 ms | 5.73 ms |
| Seeded persistent-cache review, both games, median of five | Not measured | 11.30 ms |

Native recorded-search processing used 92.6% less time, while the complete cold review used **52.2%
more time**: it performed 71 extra paired root searches. Full depth-16 and threat-depth-10 results were
retained. There were 222 native evaluation calls versus 152 baseline calls; one terminal position needs
no actual UCI search. Threat searches were 16 versus 17 because the changed score/classification model
changes which errors trigger a threat probe; the configured threat budget remains 10.
Replay equality is checked within each algorithm, not between their deliberately changed labels.
Warm-cache review started zero engine processes and performed zero UCI searches after a separately
reported 45,692.95 ms cache seed. Cold samples are one run per game, so they are not throughput statistics.

The native algorithm changes the search workload deliberately. Best and played moves are compared at
the same root and completed depth from the mover's perspective. When the played move is absent from
MultiPV, an extra paired search evaluates best and played under the same budget. Thus cold search can
take longer even when non-search work is faster. Budgets are not lowered to manufacture a speed claim.
The evaluation curve still uses separately searched next positions. Signed mates, model name,
comparison depth, base label and overlay evidence are recorded in each move's evaluation metadata.

Persistent UCI processes avoid repeated startup. Complete results are cached with engine identity,
history, depth, MultiPV, time, thread/hash and root restrictions. Cancellation is checked around cache
work; incomplete searches are not cached. The benchmark separately reports cold searches, strict
recorded-search replay and seeded warm-cache repeats. Warm replay must match stored output and start
zero engine processes/searches; it represents repeated identical review, not analysis of new games.

Base losses use inclusive boundaries at 0.02, 0.05, 0.10 and 0.20 expected points. Brilliant, Great,
Miss and Book are separate conservative overlays; see [methodology](../METHODOLOGY.md) for precedence,
evidence and limitations. These are transparent approximations, not Chess.com's proprietary labels.

## Reproduce safely

Use a new empty directory for every run, and the locked development dependencies from the README.
The fixture seeder refuses to overwrite a file; the native insight benchmark backs up its source
read-only before migrating. No command below opens the application's live database.

```sh
mkdir benchmark-output
python scripts/seed_benchmark.py benchmark-output/fixture.db
python scripts/seed_benchmark.py benchmark-output/scale100.db --scale 100
python backend/bench/run.py --source benchmark-output/fixture.db --scale 100 --repeat 5 --out benchmark-output/python-insights.json
PLYBACK_INSIGHT_BENCH_SOURCE="$PWD/benchmark-output/scale100.db" PLYBACK_INSIGHT_BENCH_OUTPUT="$PWD/benchmark-output/rust-insights.json" cargo test --release --locked --manifest-path rust/Cargo.toml --test insights benchmark_insights -- --ignored --nocapture

node scripts/install_stockfish.mjs
export PLYBACK_STOCKFISH="$(cat data/engine/stockfish.path)"
PLYBACK_DATA="$PWD/benchmark-output/engine-data" python backend/bench/analysis.py --out benchmark-output/python-engine.json --record benchmark-output/evaluations.json
PLYBACK_BENCH_INPUT="$PWD/benchmark-output/evaluations.json" PLYBACK_BENCH_OUTPUT="$PWD/benchmark-output/rust-engine.json" cargo test --release --locked --manifest-path rust/Cargo.toml --test engine_review benchmark_reference_fixture_cold_and_warm -- --ignored --nocapture
```

Run intensive measurements on an idle machine or in CI. The `Native backend (ubuntu-latest)` job
performs the same sequence and uploads `native-performance`. Compare engine timings only within
that run; hardware, OS, transposition-table state and shared-runner load affect measurements.

## Upgrade and verification boundaries

Back up first. Existing schema versions 1–5, password hashes and sessions remain readable. Appended
migrations add report membership, worker claim fencing and immutable account generations for sync.
Ready legacy report JSON is untouched;
unfinished legacy reports freeze membership on their first tick/finalization. New reports freeze
membership at creation but use current data until finalization; failed jobs or early finalization
can produce partial coverage. Restore the pre-upgrade backup to return to an older server.

Regression coverage includes label boundaries for both colours, signed mates, tactics, invalid PGNs,
partial UCI output, deadlines, HTTP disconnects, child reaping, database contention, stale job/ID reuse,
concurrent resource updates, auth rotation races, archive validation, account deletion during sync,
late uploads during shutdown, duplicate startup and fixed report membership.
Browser workflows run against native fixture servers. CI also verifies Windows/Linux builds, locked
dependency audits and a non-root read-only container with no Python or Node. Live Chess.com availability,
all possible PGNs and subjective agreement on Brilliant/Great remain outside these guarantees.
