# Architecture

Plyback is a single process: a FastAPI application that serves the REST API and the built web UI,
plus background threads for synchronisation and analysis. All state lives in one SQLite database.

```
                 ┌──────────────── Browser (React + TypeScript) ────────────────┐
                 │  Overview · Games · Review · Insights · Compare · Reports     │
                 └───────────────────────────┬───────────────────────────────────┘
                                             │ JSON over HTTP (same origin)
┌────────────────────────────────────────────▼────────────────────────────────────────────┐
│ plyback.main            app factory, security middleware, static files                  │
│ plyback.api.*           routers: system, auth, accounts, games, engine, queue, insights │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ services.sync           polls Chess.com per account (ETag/304, adaptive interval)        │
│ services.analysis       persistent priority queue → worker pool → one Stockfish each    │
│ services.reports        on-demand account analysis, snapshots when complete              │
│ services.interactive    dedicated engine for exploring positions in the UI               │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ analysis.*              PGN/clocks, engine wrapper, win%/accuracy, classification,       │
│                         tactics (SEE, forks, mates, threats), explanations, openings     │
│ insights.*              account statistics, priorities, evidence, training resources     │
│ chesscom.client         rate-limited, retrying, conditional HTTP client                  │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ db                      SQLite (WAL), per-thread connections, serialised writes,         │
│                         atomic schema + data migrations                               │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

## Data model

| Table | Purpose |
|---|---|
| `accounts`, `archives` | Connected Chess.com users and the monthly archives already fetched (ETag, completeness). |
| `games`, `account_games` | Games (shared when two connected accounts played each other) and their link to accounts with colour. |
| `game_analysis`, `moves` | Per-game summary and per-move results: evaluations, classification, tags, explanation, engine lines. |
| `jobs` | Analysis queue with priority, attempts, checkpoints (`partial_json`). |
| `position_cache` | Engine results for opening positions, reused across games. |
| `reports` | Saved account-analysis snapshots. |
| `sessions`, `settings`, `events` | Auth sessions (hashed tokens), user settings, activity log. |

## Analysis pipeline

1. **Sync** fetches new monthly archives; games are de-duplicated by UUID and their opening is identified
   immediately (no engine needed).
2. **Queue**: new games get high priority, history low priority, newest first. Jobs survive restarts;
   a job interrupted mid-game resumes from its checkpoint.
3. **Worker**: Stockfish evaluates every position (default depth 16, MultiPV 2); the analyser classifies
   moves, runs tactical checks and the null-move threat test, writes explanations and accuracy.
4. **Insights** read a covering index of the `moves` table into an bounded, per-database move cache (invalidated by
   committed analysis revision) and compute statistics on demand; results are cached per application and filter, invalidated by a transactional data revision, and served
   stale-while-revalidate while new analyses arrive. Two managed refresh workers coalesce simultaneous requests
   and finish before database shutdown.

## Concurrency

- Readers use one SQLite connection per thread (WAL mode); writers share a process-wide lock and
  `BEGIN IMMEDIATE` transactions.
- Each analysis worker owns its Stockfish process; changing worker count, threads or hash resizes the
  pool live. Interrupting a worker raises a `BaseException` subclass so best-effort error handling in
  the analyser can never swallow it.

## Extending

- New API endpoints: add a router module under `plyback/api/` and include it in `main.create_app`.
- New insight sections: add a function in `insights/compute.py` working on `G` objects and register its
  output in `compute_insights`; heavy per-move data should come from the `Move` tuple.
- Schema changes: append to `db.MIGRATIONS` (and `DATA_MIGRATIONS` for Python back-fills); bump
  `ANALYSIS_VERSION` when classification output changes so stored analyses refresh automatically.

## Delivery and operational boundaries

Supported installations run from a source checkout or the Docker image. The project metadata does not
represent a standalone wheel containing the web bundle. Use the setup scripts and locked dependency files.
The default bind is loopback; LAN/server launch requires an access password. A single instance is shared
by one person or household; accounts are chess accounts, not isolated login tenants.

SQLite backups include committed WAL content and are verified before atomic publication. Legacy upgrades
retain the original database. Shutdown cancels sync retries, joins sync/report/analysis and cache workers,
closes the interactive engine, then closes database connections. Analysis workers release their own SQLite
connections when retired during live settings changes.

The final frontend design is pending. `docs/design-explorations` contains the preserved studies; functional
repairs share existing components without selecting A, D, A+D or C.
