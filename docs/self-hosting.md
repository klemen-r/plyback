# Self-hosting

A Plyback server belongs to one person or household. Everyone who signs in shares the same accounts,
settings, games and reports; there are no separate user tenants.

There are three ways to run it. All of them keep everything in one SQLite database and need no
external services besides Chess.com and Lichess.

## Download

Each [release](https://github.com/klemen-r/plyback/releases/latest) has archives for Windows (x64),
macOS (Apple silicon) and Linux (x64 and arm64, glibc 2.35 or newer). An archive holds the server, the
interface and Stockfish, and runs where it is unpacked:

1. Unpack it anywhere you can write to.
2. Start `plyback.exe` (Windows) or `./plyback` (macOS, Linux). On macOS, first run
   `xattr -dr com.apple.quarantine .` in the folder: the builds are not notarised.
3. Open [localhost:8000](http://localhost:8000).

The database is `data/plyback.db` in the same folder. To upgrade, stop Plyback, unpack the new release
and copy `data/plyback.db` from the old folder into the new `data/` folder. `SHA256SUMS.txt` in each
release lists the archive checksums.

## Docker

```sh
docker run -d --name plyback --restart unless-stopped -p 8000:8000 -v plyback-data:/data \
  -e PLYBACK_PASSWORD='choose-a-strong-one' ghcr.io/klemen-r/plyback:3
```

Or with Compose, using [`docker-compose.yml`](../docker-compose.yml) from this repository:

```sh
PLYBACK_PASSWORD='choose-a-strong-one' docker compose up -d
```

Images are built for `linux/amd64` and `linux/arm64`. The tag `3` follows the latest 3.x release, `3.1`
its patch releases and `3.1.0` one release exactly. To upgrade, `docker compose pull && docker compose up -d`
(or pull and recreate the container); the data volume is kept.

The container listens on all interfaces, so it refuses to start until a password is saved. The first
start saves `PLYBACK_PASSWORD`; later starts don't need it, and changing the variable does not replace a
saved password (use Settings or `docker exec -it plyback plyback set-password`). The image runs as a
non-root user with a read-only root filesystem and publishes port 8000.

Data lives in the named volume `plyback-data`, which survives container updates. Always pass a named
volume (`-v plyback-data:/data`); otherwise Docker creates an anonymous volume that is easy to lose. In
Docker, host discovery sees the container's own addresses, so list the host machine's name or IP in
`PLYBACK_ALLOWED_HOSTS` when you reach the mapped port through it.

To build the image from a checkout instead, run `docker build -t plyback .`.

## From source

You need [Rust](https://rustup.rs) 1.97.1 or newer and [Node.js](https://nodejs.org) 22.12 or newer.
`setup.ps1` (Windows) or `bash setup.sh` installs locked dependencies, builds the native server and the
interface, and downloads Stockfish from its official release (pinned in `scripts/install_stockfish.mjs`
and checked against a recorded SHA-256). `start.ps1` or `bash start.sh` starts the server at
[localhost:8000](http://localhost:8000). Use `-Port 8080` / `--port 8080` for another port. To use an
installed engine instead, set `PLYBACK_STOCKFISH` to its executable.

On macOS and Linux, [just](https://just.systems/) wraps the common commands; run `just` to list them.

| Command | What it does |
|---|---|
| `just setup` | Install dependencies and Stockfish, build the interface |
| `just run` | Start the app at localhost:8000 (`--port 8080 --no-browser` to change) |
| `just status` / `just logs` | Check the server / follow the application log |
| `just password` | Set or change the access password |
| `just backup "/path/to/new-backup.db"` | Save a consistent backup (never overwrites a file) |
| `just stockfish` | Install or verify the engine |

## Network access

Any bind address other than loopback requires a password. Set one first:

```powershell
# Windows, from the repository root
.\rust\target\release\plyback.exe set-password
.\start.ps1 -Lan
```

On macOS and Linux run `rust/target/release/plyback set-password`, then `bash start.sh --lan`. From a
downloaded release, run `plyback set-password` in its folder and start it with `--host 0.0.0.0`.
On untrusted networks put Plyback behind an HTTPS reverse proxy and set `PLYBACK_SECURE_COOKIES=1`.

Host checks apply with and without a password. Loopback names and the bind address are always accepted.
With a password and a wildcard bind (`0.0.0.0` or `::`), the machine's interface addresses and hostname
are accepted too; they are read at startup, so restart after network changes. List any other name, such
as a reverse-proxy or tailnet name, in `PLYBACK_ALLOWED_HOSTS` (names only, no scheme or port).

Without a password, requests that came through a proxy (they carry `Forwarded` or `X-Forwarded-*`
headers) are refused unless their Host is listed in `PLYBACK_ALLOWED_HOSTS`. This keeps a local proxy
that rewrites Host to `127.0.0.1` from opening the server to everyone who can reach the proxy.

Behind a proxy, rate limits use the connecting IP unless that IP is listed in `PLYBACK_TRUSTED_PROXIES`
(for example `127.0.0.1,::1` for a local proxy). The proxy must overwrite or safely append
`X-Forwarded-For` or `Forwarded`. Plyback walks the chain from right to left through trusted proxies and
uses the first untrusted address. See [SECURITY.md](../SECURITY.md#running-behind-a-reverse-proxy).

## Data, upgrades and backups

Everything is stored in `plyback.db` (SQLite, WAL mode) in the data directory: `data/` next to the
program or in a source checkout, `/data` in the container, or `PLYBACK_DATA` when set.

Upgrades migrate the database on start, one transaction per step. When the analysis method changes,
games are re-reviewed in the background and the previous review stays visible until then. An older
server cannot open a newer database, so back up before upgrading.

Save a consistent backup while the server is running (the destination must not exist yet):

```sh
plyback backup /path/to/plyback-backup.db        # in a release folder or the container
rust/target/release/plyback backup /path/to/plyback-backup.db   # in a source checkout
```

Backups contain account history, analyses, settings, password hashes and sessions; keep them private.
The command checks SQLite integrity and writes the file atomically. To restore, stop Plyback, keep the
current data directory as a rollback copy, and place the backup at `plyback.db` in a fresh data
directory, together with `engine/` or a reinstalled engine. Never copy only the main file of a running
WAL database.

## Game sources

Chess.com: monthly archives are fetched once and re-checked only when they change. Errors and rate
limits back off and retry on their own.

Lichess: standard games, including games from a set position, with players, ratings, results, openings
and move clocks. UltraBullet counts as bullet, classical as rapid and correspondence as daily; variants
are skipped. Lichess's own analysis is ignored, so every review comes from Plyback's engine pipeline.
Requests run one at a time and a rate limit pauses them for at least 60 seconds. Games still in
progress are re-checked later without holding back the rest of the history.

PGN import accepts files with up to 10,000 games.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `PLYBACK_HOST` | `127.0.0.1` | Bind address |
| `PLYBACK_PORT` | `8000` | HTTP port |
| `PLYBACK_DATA` | `data/` next to the program | Database, logs and installed engine |
| `PLYBACK_STOCKFISH` | auto-detect | Stockfish executable |
| `PLYBACK_PASSWORD` | unset | Initial access password |
| `PLYBACK_SECURE_COOKIES` | unset | Send session cookies over HTTPS only |
| `PLYBACK_ALLOWED_HOSTS` | unset | Extra Host names, comma-separated |
| `PLYBACK_TRUSTED_PROXIES` | unset | Proxy IPs allowed to supply client addresses, comma-separated |
| `PLYBACK_LOG_LEVEL` | `INFO` | Log level |
| `PLYBACK_API_DOCS` | unset | `1` serves the JSON API schema at `/api/docs` |
| `PLYBACK_CHESSCOM_API` | official API | Chess.com API base, for fixtures and development |
| `PLYBACK_LICHESS_API` | official API | Lichess API base, for fixtures and development |
| `PLYBACK_FRONTEND` | `frontend/dist` next to the program | Built interface |

Engine depth, workers, threads, hash size and sync intervals are set in the app under Settings.
Settings meant for development and tests are described in [development.md](development.md).
