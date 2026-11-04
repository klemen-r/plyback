# Architecture

Plyback runs as one native Rust process. Axum serves the existing JSON API and React bundle;
managed background threads synchronise public games, analyse jobs and build reports. Stockfish
remains a separate UCI process. SQLite owns durable state. Python is retained only as a development
oracle and benchmark reference; production launchers and the container never import it.

| Layer | Owning code |
|---|---|
| HTTP, bounded blocking work, auth, static files | `rust/src/http.rs`, `security.rs` |
| CLI, startup, backup, shutdown | `rust/src/main.rs`, `config.rs` |
| Account sync, game API, queue, worker lifecycle | `rust/src/services/` |
| Persistent UCI transport and position cache | `rust/src/engine.rs` |
| PGN, legal moves, classification, tactics, explanations | `rust/src/pgn.rs`, `analysis.rs` |
| Insight computation, cache, report snapshots | `rust/src/insights/` |
| SQLite ownership and transactional upgrades | `rust/src/db.rs`, `rust/migrations/` |

## Data model and upgrades

| Table | Purpose |
|---|---|
| `accounts`, `archives` | Connected users with immutable row generations, and fetched archives with ETags. |
| `games`, `account_games` | De-duplicated games and account/colour links. |
| `game_analysis`, `moves` | Versioned summaries, evaluations, labels, explanations and lines. |
| `jobs` | Persistent priority queue, attempts and a unique ownership token for each claim. |
| `position_cache` | Bounded durable results keyed by position history, engine and search options. |
| `reports`, `report_games` | Saved snapshots and fixed membership guarded by game UUID. |
| `sessions`, `settings`, `events` | Hashed auth tokens, configuration and activity. |

Migrations 1–5 retain the earlier schema and backfills. Migration 6 adds the report membership schema;
migration 7 fences job ownership; migration 8 fences account sync against deletion and ID reuse.
Schema changes, backfills and their version update commit together.
New reports freeze membership at creation; legacy unfinished reports freeze on their first tick or
finalization. Ready legacy reports retain their stored JSON. Analysis version 7 identifies the revised scoring model;
old analyses remain readable while refresh jobs are queued. Back up before upgrading, and restore that
backup when returning to an older release.

## Analysis pipeline

1. Sync validates public API metadata and archive URLs, uses conditional requests and bounded retries,
   then inserts de-duplicated games and identifies openings without running an engine. Every response
   mutation checks the original account generation inside its transaction, including error cleanup.
2. Workers claim queued games by priority. Each claim captures a random token, game UUID and PGN;
   progress, retries and completion require that claim to still match. Deleting a game and reusing
   its integer ID cannot make an old worker overwrite its replacement.
3. Each worker reuses one Stockfish process. Best and played moves are compared from the mover's
   perspective at a common root and completed depth. If the played move is absent from MultiPV,
   a paired root search evaluates both. This additional search is a disclosed correctness cost.
4. Typed SQL rows feed insight computations. A bounded, per-database response cache uses committed
   data revisions and coalesces stale refreshes. Computation occurs outside database locks.
5. New reports freeze game IDs and UUIDs at creation. Publication checks the data revision in the
   same transaction as the saved result; completed results remain immutable. Pending reports use current
   data for surviving members; deletion removes a member, while corrected filter fields do not. Failed
   jobs or explicit early finalization can produce a ready report with partial analysis coverage.

## Concurrency and cancellation

One mutex owns each database connection. Queries return owned rows; no network or engine search
holds the connection. Short, immediate write transactions serialize changes. Engine position-cache
reads and writes retry short lock attempts (25 ms SQLite budget each) for up to 2 seconds and check
cancellation every 10 ms. Brief contention between workers therefore no longer drops completed
searches. Concurrent cold workers can still complete different engine responses for the same cache
key because their hash histories differ; a later write replaces the cached response, so a warm
re-review is not guaranteed to reproduce every concurrent cold result. A connection held longer
than the retry budget is treated as a cache miss, and cancellation never waits behind it. The response cache has 64 entries and a 32 MiB serialized-payload
limit, two concurrent refresh computations and a 120-second maximum age for stale responses.

Worker database waits poll cancellation with short lock attempts; cancelled writes roll back. If a
busy database prevents bounded shutdown cleanup, the durable claim is recovered at the next start.
The server reserves its listening port before touching data, so a duplicate launch cannot reset jobs.

HTTP permits cap concurrent blocking work. Body uploads have byte and time limits. Password hashing
runs off the async reactor and transactionally checks the verified password before issuing a session.
An API request checks session validity again after reading its body.

Live resource changes cancel and join retired workers before starting replacements, using one immutable
settings snapshot per worker generation. Workers, threads and hash memory have aggregate bounds.
Interactive analysis has a dedicated engine and a cancellation flag tied to the HTTP request lifetime.
UCI reads, output, search deadlines and retries are bounded; stubborn children are killed and reaped.
Shutdown broadcasts cancellation before draining HTTP, rejects late uploads, then joins managed workers.
Compose grants 90 seconds for a bounded network/database operation to finish. Interrupted jobs remain recoverable; durable
position results provide reuse when a game restarts, rather than resuming a Python checkpoint object.

## Extending and distribution

Add API routes to the owning native service; keep `rust/openapi.json` compatible with the frontend.
Extend `insights/compute.rs` with owned typed data, and test against the preserved oracle where behavior
should match. Append migrations and bump `ANALYSIS_VERSION` when stored results change. Update dependency
notices whenever the Cargo lock changes.

Supported installations are a source checkout and the Docker image. The binary embeds opening data,
insight resources and the API schema; launchers supply the data and built-interface locations. Network
access requires a password by default. One instance serves one person or household, not isolated tenants.
Backups use SQLite's backup API, verify integrity and publish without overwriting an existing file.

The interface follows design study A, Vercel tabs, chosen on 2026-11-03. Appearance (light/dark/system,
cover colour, board colours) is stored per browser in `frontend/src/lib/theme.ts`.
