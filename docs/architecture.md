# Architecture

Plyback runs as one native Rust process. Axum serves the JSON API and the built React interface;
managed background threads sync public games, analyse queued jobs and build reports. Stockfish runs
as a separate UCI process. SQLite holds all durable state. The Python tree in `backend/` is a
development reference and benchmark baseline; the launchers and the container never run it.

| Layer | Owning code |
|---|---|
| HTTP, bounded blocking work, auth, static files | `rust/src/http.rs`, `security.rs` |
| CLI, startup, backup, shutdown | `rust/src/main.rs`, `config.rs` |
| Account sync, game API, queue, worker lifecycle | `rust/src/services/` |
| Persistent UCI transport and position cache | `rust/src/engine.rs` |
| PGN, legal moves, classification, explanations | `rust/src/pgn.rs`, `analysis.rs`, `tactics.rs`, `board.rs` |
| Practice positions and schedule | `rust/src/practice.rs` |
| Opening tree | `rust/src/repertoire.rs` |
| Insight computation, cache, report snapshots | `rust/src/insights/` |
| SQLite access and transactional migrations | `rust/src/db.rs`, `rust/migrations/` |

## Data model

| Table | Purpose |
|---|---|
| `accounts`, `archives` | Connected users scoped by platform, each row with a fixed sync generation, and fetched Chess.com monthly archives with ETags. |
| `games`, `account_games` | De-duplicated games and account/colour links. |
| `game_analysis`, `moves` | Versioned summaries, evaluations, labels, explanations and lines. |
| `jobs` | Persistent priority queue, attempts, a unique token for each claim and pending deeper-review requests. |
| `sync_pending` | Games a sync stored for an account that still have to be queued for review. |
| `lichess_pending_games` | Ongoing Lichess game IDs waiting to be checked again. |
| `position_cache` | Durable engine results keyed by position history, engine and search options. |
| `reports`, `report_games` | Saved snapshots and their fixed game membership, checked by game UUID. |
| `practice_items` | Per-account practice schedule for positions that have been answered. |
| `sessions`, `settings`, `events` | Hashed auth tokens, configuration and activity. |
| `data_revision` | A counter that triggers bump on every change to accounts, games and analyses. |

## Migrations

`rust/migrations/` holds numbered SQL files, applied in order on start. Each migration, its backfill
and the new `schema_version` commit in one transaction. A database with a newer schema than the
running server is refused. Add a new file for every change; never edit a released migration.

| Migration | Adds |
|---|---|
| 001 | Base schema: settings, accounts, archives, games, account links, analyses, moves, position cache, jobs, reports and events. |
| 002 | `accounts.history_complete`, set for accounts that have synced before. |
| 003 | `sessions`, covering indexes for insights, and `moves.endgame_type` with a backfill from the stored positions. |
| 004 | `game_analysis.revision`, incremented each time a game is re-analysed. |
| 005 | `data_revision` and its triggers. |
| 006 | `report_games` and `reports.scope_frozen` for fixed report membership. |
| 007 | `jobs.claim_token`, so a completion only applies to the claim that started it. |
| 008 | `accounts.sync_generation`, a random value that cannot change, so a late sync response cannot write to a deleted and re-created account. |
| 009 | `practice_items`. |
| 010 | `sync_pending`, and `jobs.followup_priority` / `followup_depth` for deeper reviews requested while a job runs. |
| 011 | Platform-scoped accounts, allowing the same username on Chess.com and Lichess. The account rebuild preserves child rows, including pending review handoffs. |
| 012 | `accounts.lichess_cursor_ms`, a creation-time cursor in milliseconds. |
| 013 | Lichess scan checkpoints for bounded exports. |
| 014 | `lichess_pending_games`, so ongoing games can be checked by ID without holding back history discovery. |

The current schema version is 14.

When stored analysis results change, bump `ANALYSIS_VERSION` in `rust/src/lib.rs` (currently 7).
On start, games analysed with an older version are queued for refresh; their old analysis stays
readable until the new one is stored.

Reports created now freeze their game IDs and UUIDs at creation. Reports from before migration 006
freeze on their first update or on finalization if unfinished; ready ones keep their stored JSON.

## Analysis pipeline

1. Chess.com sync validates public API metadata and archive URLs, sends conditional requests, makes up to
   six attempts with exponential back-off, then stores de-duplicated games and names their openings
   without running an engine. Lichess sync streams finished standard-chess games by creation time,
   checkpoints bounded exports and checks ongoing game IDs separately. Requests are serialized, and
   HTTP 429 pauses requests for at least 60 seconds. Each write checks the account's sync generation
   in the same transaction.
   New games go to `sync_pending` in the archive's transaction and leave it in the transaction that
   queues them, so an interrupted sync resumes with an exact list.
2. Workers claim queued games by priority. Each claim records a random token, the game UUID and PGN;
   progress, retries and completion only apply while that claim still matches.
3. Each worker reuses one Stockfish process. Best and played moves are compared from the mover's
   side at the same root and completed depth. If the played move is not among the MultiPV lines,
   one extra paired search evaluates both (see [methodology](../METHODOLOGY.md#1-engine-evaluation)).
4. Insights read typed SQL rows. A per-database response cache is keyed on `data_revision`, serves
   recent stale results while it refreshes, and computes outside the database lock.
5. Report publication checks the data revision in the same transaction as the saved result; a ready
   report never changes. Pending reports use current data for surviving members. Deleting a game
   removes it from a report; corrected filter fields do not. Failed jobs or early finalization can
   produce a ready report with partial analysis coverage.

## Concurrency and cancellation

- One mutex guards each database connection. Queries return owned rows, so no network call or
  engine search holds the connection. Writes use short immediate transactions.
- Position-cache reads and writes are optional. They retry 25 ms lock attempts for up to 2 seconds,
  checking cancellation every 10 ms; after that the cache is treated as a miss.
- Workers that share a database share in-flight searches with the same full cache key (history,
  engine, threads, hash and all search limits). A follower gets the leader's result without
  searching. A cancelled follower leaves the leader running; a cancelled or failed leader releases
  its followers to search with their own engines. This is scoped to one `Database` instance.
- The position cache is trimmed to 20,000 entries. The insight response cache holds 64 entries and
  32 MiB, runs at most two refreshes at once and serves stale results for up to 120 seconds.
- Changing worker settings cancels and joins the old workers before starting new ones. Workers,
  threads and hash memory have aggregate limits.
- Interactive analysis has its own engine and is cancelled when the HTTP request ends. UCI reads,
  search deadlines and retries are bounded; unresponsive engine processes are killed and reaped.
- The server binds its port before opening the database, so a second launch on the same port cannot
  reset the running instance's job claims.
- Shutdown signals cancellation, drains HTTP, rejects new uploads, then joins workers. Compose allows
  90 seconds. An interrupted job is claimable again at the next start and reuses saved position results.

## HTTP and security

- A semaphore caps concurrent blocking work. Request bodies are limited to 25 MiB and 15 seconds.
- Password hashing runs off the async runtime. Login checks the verified password hash
  again in the transaction that issues the session. An API request checks its session again after reading the body.
- Host validation applies with or without a password. Protected wildcard listeners also accept the
  interface IPs and OS hostname seen at startup; other names go in `PLYBACK_ALLOWED_HOSTS`.
- Rate limits use the TCP peer. `PLYBACK_TRUSTED_PROXIES` lists proxy IPs whose `X-Forwarded-For` or
  `Forwarded` chain is read right to left; a malformed chain falls back to the peer. See
  [proxy configuration](../SECURITY.md#running-behind-a-reverse-proxy).
- Built assets have pre-compressed Brotli and gzip variants, chosen by `Accept-Encoding` with
  `Vary: Accept-Encoding`. Errors are never cached as immutable assets. API responses are uncompressed
  and `no-store`.

## Interface

The interface follows design study A, Vercel tabs, chosen on 2026-11-03. Appearance (light/dark/system,
cover colour, board colours) is stored per browser in `frontend/src/lib/theme.ts`.

The shell polls status every 5 seconds while reviews or a sync are running and every 15 seconds when
idle. A change in `data_revision` refreshes account, game, insight and opening queries, including
re-reviews that finish between polls. Insights poll only while stale or after an error. Practice marks
its queue stale without replacing the current puzzle, and late answers or engine checks cannot update a
skipped puzzle or another account.

## Extending and distribution

Add API routes to the owning service and keep `rust/openapi.json` in step with the frontend. Extend
`insights/compute.rs` with owned typed data. Append migrations and bump `ANALYSIS_VERSION` when stored
results change. Regenerate dependency notices whenever `rust/Cargo.lock` changes.

Supported installations are a source checkout and the Docker image. The binary embeds opening data,
insight resources and the API schema; launchers supply the data and built-interface locations. Network
access requires a password by default. One instance serves one person or household, not isolated users.
Backups use SQLite's backup API, verify integrity and never overwrite an existing file.

CI passes its read-only token only to the official Stockfish release metadata API; container builds get
it as a BuildKit secret. Download checksums are always checked.

## Benchmarks

Two ignored Rust tests measure the app. `benchmark_insights` (in `rust/tests/insights.rs`) times
uncached insight computation on a seeded database. `benchmark_reference_fixture_cold_and_warm`
(in `rust/tests/engine_review.rs`) reviews two fixture games with the default budget and reports
cold searches, an exact replay of the recorded searches, and repeats against a seeded position
cache; the warm repeats must match the stored output and start no engine process or search. The
engine benchmark replays evaluations recorded by the Python baseline, so it needs the development
dependencies from the README.

Use a new empty directory for every run. The seeder refuses to overwrite a file, and the insight
benchmark backs up its source read-only before migrating. No command below opens the live database.

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

Run them on an idle machine or in CI. The `Native backend (ubuntu-latest)` job runs the same sequence
on pushes to `main` and on manual runs, and uploads the results as `native-performance`. Compare timings
only within one run: hardware, OS, engine hash state and shared-runner load all affect them.
