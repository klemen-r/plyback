"""Measure native page APIs on a disposable SQLite backup; emit no response data.

Run with --source /path/to/snapshot.db --out benchmark-output/api.json.
Supply a self-contained backup, never a live database. Background workers and
external sync are disabled. Only an OS temporary copy is opened by the server.
"""
import argparse
import hashlib
import http.client
import json
import os
import shutil
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from pathlib import Path


def snapshot_copy(source: Path, destination: Path) -> str:
    """Copy a self-contained snapshot without opening or creating WAL sidecars."""
    source = source.resolve(strict=True)
    if any(Path(str(source) + suffix).exists() for suffix in ("-wal", "-shm", "-journal")):
        raise ValueError("Supply a self-contained SQLite backup, not an active database")
    with source.open("rb") as source_file:
        digest = hashlib.file_digest(source_file, "sha256").hexdigest()
    with (
        closing(sqlite3.connect(source.as_uri() + "?mode=ro&immutable=1", uri=True)) as original,
        closing(sqlite3.connect(destination)) as target,
    ):
        original.backup(target)
    return digest


def write_result(path: Path, result: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation also protects against a path appearing during the run.
    with path.open("x", encoding="utf-8") as output:
        output.write(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=15)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--frontend", type=Path)
    parser.add_argument("--browser", action="store_true", help="measure fresh Chromium page asset transfers")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    if args.out.exists() or args.out.is_symlink():
        parser.error("--out must be a new file; existing files are never overwritten")
    browser_node = shutil.which("node") if args.browser else None
    if args.browser and browser_node is None:
        parser.error("--browser requires Node.js on PATH")
    root = Path(__file__).resolve().parents[1]
    binary = args.binary or root / "rust/target/release" / ("plyback.exe" if os.name == "nt" else "plyback")
    with tempfile.TemporaryDirectory(prefix="plyback-api-bench-") as directory:
        data = Path(directory)
        original_hash = snapshot_copy(args.source, data / "plyback.db")
        with closing(sqlite3.connect(data / "plyback.db")) as target:
            first_account = target.execute("SELECT id FROM accounts ORDER BY id LIMIT 1").fetchone()
            first_game = target.execute("SELECT game_id FROM game_analysis ORDER BY game_id LIMIT 1").fetchone()
            if first_account is None or first_game is None:
                parser.error("snapshot must contain an account and a reviewed game")
            account, game = first_account[0], first_game[0]
            counts = {table: target.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
                      for table in ("accounts", "games", "moves")}
            # Local benchmark only: preserve the source password and sessions untouched.
            target.execute("DELETE FROM settings WHERE key='auth_password_hash'")
            target.commit()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if not k.startswith(("PLYBACK_", "CHESS_INSIGHT_"))}
        env.update(PLYBACK_DATA=str(data), PLYBACK_HOST="127.0.0.1", PLYBACK_PORT=str(port),
                   PLYBACK_NO_BACKGROUND="1", PLYBACK_FRONTEND=str((args.frontend or root / "frontend/dist").resolve()))
        if "PLYBACK_STOCKFISH" in os.environ:
            env["PLYBACK_STOCKFISH"] = os.environ["PLYBACK_STOCKFISH"]
        started = time.perf_counter()
        with (data / "server.log").open("w") as log:
            process = subprocess.Popen([str(binary.resolve())], env=env, stdout=log, stderr=log)
            try:
                clients = threading.local()

                def request(path):
                    if not hasattr(clients, "connection"):
                        clients.connection = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
                    connection = clients.connection
                    begin = time.perf_counter()
                    try:
                        connection.request("GET", path)
                        response = connection.getresponse()
                        payload = response.read()
                        elapsed = (time.perf_counter() - begin) * 1000
                        if response.status != 200:
                            raise RuntimeError(f"Benchmark request failed: {response.status}")
                        return elapsed, payload
                    except Exception:
                        connection.close()
                        raise

                while True:
                    try:
                        request("/api/health")
                        break
                    except OSError:
                        if process.poll() is not None or time.perf_counter() - started > 30:
                            raise RuntimeError("Benchmark server did not start") from None
                        time.sleep(0.01)
                result = {"startup_ms": round((time.perf_counter() - started) * 1000, 3),
                          "workload": counts, "repeat": args.repeat, "endpoints": {}}
                paths = {
                    "session": "/api/auth/session", "status": "/api/status", "events": "/api/events",
                    "accounts": "/api/accounts", "queue": "/api/queue",
                    "games": f"/api/games?account_id={account}",
                    "game": f"/api/games/{game}?account_id={account}",
                    "opening_filters": f"/api/games/openings?account_id={account}",
                    "insights": f"/api/accounts/{account}/insights",
                    "practice": f"/api/accounts/{account}/practice",
                    "repertoire_white": f"/api/accounts/{account}/repertoire?color=white",
                    "repertoire_black": f"/api/accounts/{account}/repertoire?color=black",
                    "reports": f"/api/accounts/{account}/reports",
                    "settings": "/api/settings",
                }
                for name, path in paths.items():
                    cold, payload = request(path)
                    samples = [request(path)[0] for _ in range(args.repeat)]
                    value = json.loads(payload)
                    if isinstance(value, dict):
                        value.pop("generated_at", None)
                        if name == "status":
                            # Operational metadata, not page/review output.
                            value.pop("data_revision", None)
                            value.pop("db_size", None)
                    result["endpoints"][name] = {
                        "first_ms": round(cold, 3), "median_ms": round(statistics.median(samples), 3),
                        "p95_ms": round(sorted(samples)[int((len(samples) - 1) * 0.95)], 3),
                        "bytes": len(payload),
                        "sha256": hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest(),
                    }
                # Concurrent navigation/polling exposes contention on the shared database.
                batch = [paths[key] for key in ("practice", "repertoire_white", "insights", "games", "game", "status")]
                with ThreadPoolExecutor(max_workers=6) as pool:
                    samples = list(pool.map(lambda path: request(path)[0], batch * 5))
                result["concurrent"] = {"median_ms": round(statistics.median(samples), 3),
                                        "max_ms": round(max(samples), 3)}
                if args.browser:
                    measured = subprocess.run(
                        [browser_node, str(root / "frontend/scripts/measure-load.mjs"),
                         f"http://127.0.0.1:{port}", str(game)],
                        capture_output=True, text=True, check=True, timeout=180,
                    )
                    result["first_load"] = json.loads(measured.stdout)
                with args.source.open("rb") as source_file:
                    if hashlib.file_digest(source_file, "sha256").hexdigest() != original_hash:
                        raise RuntimeError("Source snapshot changed during benchmark")
                result["snapshot_unchanged"] = True
                write_result(args.out, result)
                print(json.dumps(result, indent=2))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
