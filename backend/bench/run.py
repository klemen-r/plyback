"""Performance benchmarks.

    python bench/run.py [--source DB] [--scale 20] [--out results.json]

Builds a scaled copy of an existing database (each analysed game is cloned ``scale``
times with fresh ids), then measures the hot paths: insights, the main API endpoints,
the analysis pipeline overhead (engine excluded) and opening-index start-up.
"""

from __future__ import annotations

import argparse
import cProfile
import json
import logging
import os
import platform
import sqlite3
import statistics
import sys
import tempfile
import time
import uuid
from contextlib import closing
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


def timeit(fn, repeat=5):
    xs = []
    for _ in range(repeat):
        t = time.perf_counter()
        fn()
        xs.append((time.perf_counter() - t) * 1000)
    return {"median_ms": round(statistics.median(xs), 1), "min_ms": round(min(xs), 1)}


def scale_db(src: Path, dst: Path, factor: int) -> None:
    if factor < 1:
        raise ValueError("scale must be at least 1")
    if not src.is_file():
        raise FileNotFoundError(src)
    if dst.exists():
        raise FileExistsError(dst)
    # SQLite backup includes committed WAL pages and gives one consistent snapshot
    # while the running application continues writing to its own database.
    with (
        closing(sqlite3.connect(src.resolve().as_uri() + "?mode=ro", uri=True)) as source,
        closing(sqlite3.connect(dst)) as c,
    ):
        source.backup(c)
        c.row_factory = sqlite3.Row
        cols_g = [r[1] for r in c.execute("PRAGMA table_info(games)") if r[1] != "id"]
        cols_m = [r[1] for r in c.execute("PRAGMA table_info(moves)") if r[1] != "game_id"]
        cols_a = [r[1] for r in c.execute("PRAGMA table_info(game_analysis)") if r[1] != "game_id"]
        base = c.execute(
            "SELECT g.id FROM games g JOIN game_analysis ga ON ga.game_id=g.id "
            "WHERE EXISTS (SELECT 1 FROM account_games ag WHERE ag.game_id=g.id)"
        ).fetchall()
        c.execute("BEGIN")
        for k in range(1, factor):
            shift = k * 86400 * 40
            for (gid,) in base:
                g = dict(c.execute("SELECT * FROM games WHERE id=?", (gid,)).fetchone())
                g["uuid"] = str(uuid.uuid4())
                for timestamp in ("start_time", "end_time"):
                    if g[timestamp] is not None:
                        g[timestamp] -= shift
                new = c.execute(
                    f"INSERT INTO games ({','.join(cols_g)}) VALUES ({','.join('?' * len(cols_g))})", [g[x] for x in cols_g]
                ).lastrowid
                c.execute(
                    "INSERT INTO account_games (account_id, game_id, color) "
                    "SELECT account_id, ?, color FROM account_games WHERE game_id=?", (new, gid)
                )
                a = dict(c.execute("SELECT * FROM game_analysis WHERE game_id=?", (gid,)).fetchone())
                c.execute(
                    f"INSERT INTO game_analysis (game_id,{','.join(cols_a)}) VALUES (?,{','.join('?' * len(cols_a))})",
                    [new] + [a[x] for x in cols_a],
                )
                for m in c.execute("SELECT * FROM moves WHERE game_id=?", (gid,)).fetchall():
                    m = dict(m)
                    c.execute(
                        f"INSERT INTO moves (game_id,{','.join(cols_m)}) VALUES (?,{','.join('?' * len(cols_m))})",
                        [new] + [m[x] for x in cols_m],
                    )
        c.execute("DELETE FROM jobs")
        c.execute("COMMIT")
        c.execute("VACUUM")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", default=str(ROOT.parent / "data" / "plyback.db"))
    ap.add_argument("--scale", type=int, default=20)
    ap.add_argument("--out")
    ap.add_argument("--repeat", type=int, default=5, help="repetitions for warm timings")
    ap.add_argument("--profile", help="write a separate warm insights cProfile capture")
    args = ap.parse_args()
    if args.scale < 1 or args.repeat < 1:
        ap.error("--scale and --repeat must be at least 1")
    src = Path(args.source)
    if not src.exists():
        legacy = src.with_name("chess-insight.db")
        src = legacy if legacy.exists() else src
    with tempfile.TemporaryDirectory(prefix="plyback-bench-") as directory:
        try:
            results = run(args, src, Path(directory))
        finally:
            # Release the application's rotating log before Windows removes the sandbox.
            logging.shutdown()
    print(json.dumps(results, indent=2))
    if args.out:
        Path(args.out).write_text(json.dumps(results, indent=2), encoding="utf-8")


def run(args, src: Path, tmp: Path) -> dict:
    os.environ["PLYBACK_DATA"] = os.environ["CHESS_INSIGHT_DATA"] = str(tmp)
    db_path = tmp / "bench.db"
    t = time.perf_counter()
    scale_db(src, db_path, args.scale)
    results: dict = {
        "python": platform.python_version(),
        "platform": platform.system(),
        "sqlite": sqlite3.sqlite_version,
        "scale": args.scale,
        "repeat": args.repeat,
        "build_db_s": round(time.perf_counter() - t, 1),
    }

    from fastapi.testclient import TestClient

    from plyback import security
    from plyback.analysis import openings
    from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
    from plyback.analysis.pgn import parse_pgn
    from plyback.main import create_app

    t = time.perf_counter()
    openings._index()
    results["openings_index_ms"] = round((time.perf_counter() - t) * 1000, 1)

    app = create_app(db_path, start_background=False, serve_frontend=False)
    with TestClient(app, base_url="http://127.0.0.1", headers={"X-Requested-With": "plyback"}) as client:
        db = app.state.services.db
        # Authenticate only against the temporary copy, including password-protected sources.
        client.cookies.set(security.SESSION_COOKIE, security.create_session(db, "Plyback benchmark"))
        results["games"] = db.scalar("SELECT COUNT(*) FROM games")
        results["moves"] = db.scalar("SELECT COUNT(*) FROM moves")
        acc = db.scalar("SELECT MIN(id) FROM accounts")
        gid = db.scalar("SELECT MAX(game_id) FROM game_analysis")
        if acc is None or gid is None:
            raise ValueError("benchmark source must contain an account and analysed games")

        from plyback.insights.compute import compute_insights

        t = time.perf_counter()
        compute_insights(db, acc, {})
        results["insights_cold_ms"] = round((time.perf_counter() - t) * 1000, 1)
        results["insights_all_ms"] = timeit(lambda: compute_insights(db, acc, {}), args.repeat)
        results["insights_blitz_white_ms"] = timeit(
            lambda: compute_insights(db, acc, {"time_classes": "blitz", "color": "white"}), args.repeat
        )
        results["api_games_page_ms"] = timeit(
            lambda: client.get("/api/games", params={"account_id": acc, "page_size": 30}).raise_for_status()
        )
        results["api_games_filtered_ms"] = timeit(
            lambda: client.get(
                "/api/games", params={"account_id": acc, "outcome": "loss", "classification": "blunder"}
            ).raise_for_status()
        )
        results["api_game_detail_ms"] = timeit(lambda: client.get(f"/api/games/{gid}").raise_for_status())
        results["api_accounts_ms"] = timeit(lambda: client.get("/api/accounts").raise_for_status())
        results["api_status_ms"] = timeit(lambda: client.get("/api/status").raise_for_status())
        results["api_insights_cached_ms"] = timeit(lambda: client.get(f"/api/accounts/{acc}/insights").raise_for_status())

        # Analysis pipeline cost excluding the engine: replay the engine lines stored in the database.
        from plyback.analysis.engine import Line

        sample = db.query(
            "SELECT g.id, g.pgn FROM games g JOIN game_analysis ga ON ga.game_id = g.id ORDER BY g.id LIMIT 40"
        )
        recorded: dict[str, list] = {}
        for g in sample:
            for m in db.query("SELECT fen_before, lines_json FROM moves WHERE game_id=?", (g["id"],)):
                lines = json.loads(m["lines_json"] or "[]")
                recorded[" ".join(m["fen_before"].split()[:4])] = [
                    Line(x["cp"], [x["uci"]] if x["uci"] else [], 16) for x in lines
                ]

        def replay(board, depth, multipv):
            return recorded.get(board.epd(), [Line(0, [], depth)])[:multipv] or [Line(0, [], depth)]

        parsed = [parse_pgn(g["pgn"]) for g in sample]
        r = timeit(lambda: [analyze_parsed(p, replay, AnalysisOptions(depth=16)) for p in parsed], 1)
        results["pipeline_overhead_ms_per_game"] = round(r["median_ms"] / len(parsed), 1)
        if args.profile:
            profile = cProfile.Profile()
            profile.runcall(compute_insights, db, acc, {})
            profile.dump_stats(args.profile)
    return results


if __name__ == "__main__":
    main()
