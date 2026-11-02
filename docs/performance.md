# Performance verification

The separate [Stockfish installer measurement](installer-performance.md) records
archive-extraction memory, download integrity and installation failure recovery.

Measured on Windows with Python 3.14.3 and SQLite 3.50.4 on 2026-11-02.
The workload was a consistent SQLite backup of 205 analysed games and 11,345 moves,
scaled 20 times to 4,100 games and 226,900 moves. Scaling repeats existing games;
it exercises storage and aggregation size without representing a new set of games.
The live database was opened read-only for SQLite's backup API and was never migrated,
scaled or analysed by the benchmark.

## Optimisation result

Profiling identified repeated scans in time-management statistics, phase-rate
aggregation and hourly results. Time statistics now collect phase timings, clock
curves, time trouble and speed buckets in one pass over each player's moves.
Phase rates accumulate their statistics together. Hourly results convert each
timestamp once, rather than once for each of the 24 hours.

A paired comparison alternated the previous and optimised aggregation functions in
the same process and database, excluded one warm-up, and measured seven samples per
implementation. The complete insights dictionaries were equal on every pair.

| Warm insights computation | Median |
|---|---:|
| Previous aggregations | 1,761.3 ms |
| Optimised aggregations | 1,607.6 ms |

This is an **8.7% reduction** in median computation time for this workload. Raw
samples are in [performance-results.json](performance-results.json). Separate whole
application runs showed larger differences while other engineering checks were
running; those differences are not attributed to the code changes.

Move caches are bounded and scoped to their database. A committed per-game revision
invalidates them even when multiple analyses finish in the same second. Result
caching is separate from these computation timings.

## Running benchmarks

From the repository root on Windows:

```powershell
.\.venv\Scripts\python.exe backend/bench/run.py --scale 20 --repeat 5 --out results.json --profile insights.prof
.\.venv\Scripts\python.exe -m pstats insights.prof
```

On Linux or macOS, use `.venv/bin/python`. Pass `--source /path/to/plyback.db` for
a different database. The source must contain at least one connected account and
analysed game. The tool supports password-protected sources by creating a session
only in the temporary copy. It refuses to overwrite existing files, preserves
shared-game account links, includes committed WAL pages, and cleans up its working
database after success or failure.

The JSON records dataset size, runtime versions, cold and warm insight timings,
filtered insights, representative API requests, opening-index loading, and analysis
pipeline overhead using recorded engine responses. `--profile` captures one extra
warm computation after the timings, so profiler overhead is excluded from them.
These measurements do not establish Stockfish throughput or production capacity.

## Game-analysis pipeline

Real Stockfish 19 runs used one thread, 64 MB hash, depth 16, MultiPV 2, the normal
5-second position cap and depth-10 threat searches. The two fixture games contained
88 and 45 plies. Every nonterminal main position reached depth 16. Engine search
accounted for more than 99% of elapsed analysis time in these runs; removing Python
overhead alone cannot make uncached games instantaneous.

The analyser now converts each position's principal variations to SAN once and
reuses them for alternatives, explanations and the preceding move's reply. FEN
strings are shared between adjacent moves. Material counts use existing bitboards,
and static exchange evaluation generates only legal moves onto its target square.
All tactical, sacrifice, missed-mate and null-move threat checks remain enabled.

For a paired replay of all recorded engine responses for both games, ten alternating
samples gave these results. PGN parsing and real engine search were excluded from
the replay timings. Every engine request and every output field other than the
elapsed-time measurement matched.

| Pipeline work for 133 plies | Previous | Optimised |
|---|---:|---:|
| Median Python processing | 264.28 ms | 150.81 ms |
| PV-to-SAN conversions | 665 | 269 |
| Median peak traced Python allocations, three runs | 574,963 bytes | 512,500 bytes |

This reduces Python processing time by **42.9%** and its measured allocation peak by
**10.9%** for this replay workload. These percentages apply to post-processing,
not total analysis latency or operating-system memory use.

### Real persistent-cache reuse

`bench/service_cache.py` ran the 45-ply fixture through the actual analysis queue
twice in a new temporary database on an Intel Core i5-9400F. Each pass used a fresh
Stockfish process and empty engine hash. Both used depth 16 / MultiPV 2 / 5 seconds,
and both ran ten threat searches. Process startup was excluded from the job timer.

| Actual service run | Cold cache | Forced repeat |
|---|---:|---:|
| Wall time | 31.198 s | 10.128 s |
| Real UCI searches | 55 | 24 |
| Nodes searched | 14,333,388 | 4,185,482 |
| Achieved main-search depth, nonterminal | 16 | 16 |

The repeat reused 31 persisted position evaluations and took **67.5% less wall time**
in this measurement. This verifies the existing reuse scenario; it is not a universal
speedup for new games or a before/after result for the Python changes. The benchmark
explicitly forces the repeat: normal queueing was verified to skip this completed,
current-depth analysis entirely. Separate engine searches may produce slightly
different lines because their transposition-table histories differ; exact-output
equivalence above uses identical recorded responses.

Reproduce with:

```powershell
.\.venv\Scripts\python.exe backend/bench/service_cache.py --out cache-results.json
.\.venv\Scripts\python.exe backend/bench/analysis.py --out analysis.json --record evaluations.json
.\.venv\Scripts\python.exe backend/bench/analysis.py --replay evaluations.json --profile analysis.prof
```

The engine benchmark uses fixture PGNs by default, accepts `--pgn games.pgn`, and
records actual nodes, requested/achieved depths, engine time and replay overhead.
Recording files contain PGNs and engine lines. None of these commands opens or
changes the live application database.

An additional explicitly node-capped experiment (`--nodes 100000`) took 14.7 and
9.0 seconds for the two games, but some main searches stopped below depth 16.
It measures a different search budget and is **not enabled in production** or used
as evidence of equivalent-quality acceleration. Full raw results are retained in
[performance-results.json](performance-results.json).
