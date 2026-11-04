# Plyback

Self-hosted Chess.com game review and account insights, powered by Stockfish.
Connect public Chess.com usernames or import PGN files, review games on an interactive board,
compare selections, and save account analysis snapshots. Public game data and analyses stay in SQLite.
No Chess.com password or paid service is needed.

The interface follows the Vercel-style direction (study A) from the
[design explorations](docs/design-explorations/README.md).

## Run locally

Builds with **Rust 1.97.1+** and **Node.js 22.12+**. The production server is a native Rust
executable; running the built app requires neither Python nor Node.

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File setup.ps1
.\start.ps1
```

macOS / Linux:

```sh
bash setup.sh
bash start.sh
```

Open [localhost:8000](http://localhost:8000). The setup installs locked dependencies, builds the
interface and downloads Stockfish from its official release. To use an installed engine instead,
set `PLYBACK_STOCKFISH` to its executable path. Settings control depth, workers, threads, hash and sync.
Use `start.ps1 -Port 8080` or `bash start.sh --port 8080` to choose another port.

### Command shortcuts (macOS / Linux)

If [just](https://just.systems/) is installed (`brew install just` on macOS), run these
from the repository directory. `just` by itself lists all commands.

| Command | What it does |
|---|---|
| `just setup` | Install locked dependencies, Stockfish and build the interface |
| `just setup-dev` | Set up the app plus development tools and Chromium |
| `just run` | Start the app at localhost:8000; Ctrl+C stops it |
| `just run --port 8080 --no-browser` | Choose a port and skip opening a browser |
| `just status` | Check whether the local server is responding |
| `just logs` | Follow the application log |
| `just dev` | Start the interface with live updates at localhost:5173; run `just run` in another terminal |
| `just build` | Build the native server and interface |
| `just lint` / `just test` | Check code style / run backend and interface unit tests |
| `just e2e` | Run browser tests against separate test data and local fixtures |
| `just check` | Run lint, unit tests, build and browser tests in order |
| `just password` | Set or change the access password interactively |
| `just backup "/path/to/new-backup.db"` | Save a consistent database backup; never overwrite an existing file |
| `just stockfish` | Install or verify the engine |
| `just audit` | Check the locked dependencies for known vulnerabilities |
| `just update-deps` | Refresh compatible dependencies, install them and run `just check` |

Run `just setup-dev` before using all checks. Python 3.11+ is needed only for the preserved
baseline's development tests and benchmark oracle. `just update-deps` also needs
[uv](https://docs.astral.sh/uv/) for those development locks. Review dependency and license
changes before committing. Windows users can use the PowerShell scripts and the commands below.

## Network and Docker deployments

A server belongs to one person or household. All signed-in users share the same accounts, settings,
games and reports. This is not a service with isolated user tenants.

Set a password before enabling network access:

```powershell
# Windows, from the repository root
.\rust\target\release\plyback.exe set-password
.\start.ps1 -Lan
```

On macOS/Linux use `rust/target/release/plyback set-password` from the repository root, then
`bash start.sh --lan` from the root. Use an HTTPS reverse proxy on untrusted networks and set
`PLYBACK_SECURE_COOKIES=1`. See [SECURITY.md](SECURITY.md).

For Docker, export a strong `PLYBACK_PASSWORD` and run:

```sh
docker compose up -d --build
```

The non-root container stores data in `plyback-data`, has a read-only root filesystem, and publishes
port 8000. Changing the environment password after initial setup does not replace a saved password;
use Settings or the password command. The image is built for the host architecture.

## What it does

- Incremental Chess.com imports with conditional requests, bounded retries and adaptive polling.
- Persistent analysis queue with priorities, reusable position results, retries and independent worker engines.
- Move evaluation, classifications, tactical explanations, opening transpositions and key moments.
- Keyboard board navigation, evaluation graphs, engine lines and interactive exploration.
- Filters by period, time control, colour, result, rated status, opening and move classification.
- Account insights for openings, phases, recurring mistakes, time management, conversion and sessions,
  with confidence labels and links to evidence positions and training resources.
- Practice: positions from your own mistakes with one clear answer, on a spaced-repetition schedule.
- Side-by-side comparisons and immutable saved report snapshots.
- Password sessions, CSRF checks, body limits, outbound URL validation and browser security headers.

Accuracy and classifications use documented heuristics, not Chess.com's proprietary algorithms.
They can differ from Chess.com and depend on engine depth/budget. Insights describe the selected
sample; they are not proof of causation or coaching outcomes. See [METHODOLOGY.md](METHODOLOGY.md).

## Data, upgrades and backups

Default storage is `data/plyback.db`, with SQLite WAL enabled. On first start, an existing
`data/chess-insight.db` is copied using SQLite's backup API, including committed WAL content.
The original is retained. Schema migrations and their data backfills are transactional.
Version 3 adopts the existing database, password and sessions in place; appended migrations support
fixed report membership and fence analysis job ownership. Existing analyses remain available while version 7
analyses are queued for refresh. Back up before upgrading; use that backup if returning to an older server.

Save a consistent backup while the service is running (destination must not already exist):

```powershell
.\rust\target\release\plyback.exe backup D:\Backups\plyback-2026-11-02.db
```

On macOS/Linux use `rust/target/release/plyback backup /path/to/backup.db` from the repository root.
Backups include account history, analyses, settings, password hashes and sessions; protect them as
private data. The command verifies SQLite integrity and publishes the snapshot atomically.
To restore, stop Plyback, retain the current data directory as a rollback copy, and put the backup
at `data/plyback.db` in a fresh data directory. Copy or reinstall `data/engine` as needed.
Do not copy only the main database file from a running WAL database.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `PLYBACK_HOST` | `127.0.0.1` | Bind address |
| `PLYBACK_PORT` | `8000` | HTTP port |
| `PLYBACK_DATA` | repository `data/` | Database, logs and installed engine |
| `PLYBACK_STOCKFISH` | auto-detect | Explicit Stockfish executable |
| `PLYBACK_PASSWORD` | unset | Initial access password |
| `PLYBACK_SECURE_COOKIES` | unset | Require HTTPS for session cookies |
| `PLYBACK_ALLOWED_HOSTS` | loopback only | Extra host names for a password-less instance |
| `PLYBACK_LOG_LEVEL` | `INFO` | Logging level |
| `PLYBACK_API_DOCS` | unset | `1` enables `/api/docs`, linking to the JSON API schema |
| `PLYBACK_CHESSCOM_API` | official API | Override for fixtures/development |
| `PLYBACK_FRONTEND` | `frontend/dist` | Built interface location |

Legacy `CHESS_INSIGHT_DATA`, `CHESS_INSIGHT_FRONTEND`, `CHESSCOM_API_BASE`, `STOCKFISH_PATH`,
`HOST`, `PORT` and `LOG_LEVEL` remain supported. Remote unauthenticated access requires the explicit
`PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE=1` override; do not use it on an untrusted network.

## Development and verification

```powershell
node scripts/install_stockfish.mjs
node --test scripts/install_stockfish.test.mjs
cargo fmt --manifest-path rust/Cargo.toml --check
cargo clippy --locked --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --release --locked --manifest-path rust/Cargo.toml
cargo build --release --locked --manifest-path rust/Cargo.toml
python -m venv .venv
.venv\Scripts\python -m pip install --require-hashes -r backend/requirements-dev.txt
cd backend
..\.venv\Scripts\python -m ruff check plyback tests bench
..\.venv\Scripts\python -m pytest -m "not live"
cd ../frontend
npm ci
npm run lint
npm test
npm run build
npx playwright install chromium
npm run e2e
```

Use `.venv/bin/python` and `../.venv/bin/python` on macOS/Linux. Real-engine tests require Stockfish;
CI installs it. `pytest -m live` opts into public Chess.com network checks. Tests use isolated data,
not the user's database. CI verifies Rust on Linux/Windows, the preserved Python oracle, browser
workflows against Rust, dependency audits and the container. The Python tree is retained as a
compatibility reference; setup scripts and Docker never run it.
[Native migration and performance](docs/native-migration.md) documents measurements, methodology
changes and operational limits; [earlier measurements](docs/performance.md) remain available.

Rust uses `rust/Cargo.lock`; regenerate its bundled licenses with `node scripts/rust_notices.mjs`
after updating it. Python development dependencies are resolved across platforms with hashes in `backend/requirements*.txt`;
source constraints live in `backend/pyproject.toml`. Regenerate with `uv pip compile --python-version 3.11 --universal
--generate-hashes`, adding `--extra dev --constraint backend/requirements.txt` for the development
lock. JavaScript uses `npm ci` and its lockfile. `just update-deps` refreshes these dependency sets
and runs the verification commands above.

See [architecture](docs/architecture.md), [contributing](CONTRIBUTING.md) and [security](SECURITY.md).

## License

GPL-3.0-or-later. Keep the [license](LICENSE) and [third-party notices](THIRD_PARTY_NOTICES.md)
when redistributing. Stockfish is downloaded separately from its official release; opening data
comes from the CC0 Lichess chess-openings dataset.
