"""Measure actual queue/cache reuse with fresh Stockfish processes on a temporary DB.

    python bench/service_cache.py --out cache-results.json

Runs one fixture game twice at standard settings. The second job is explicitly
forced for measurement; normal queueing skips an already current analysis.
"""

from __future__ import annotations

import argparse
import json
import platform
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from plyback.analysis.engine import Engine  # noqa: E402
from plyback.db import Database  # noqa: E402
from plyback.services import store  # noqa: E402
from plyback.services.analysis import AnalysisService  # noqa: E402


class Probe:
    def __init__(self, engine: Engine):
        self.engine = engine
        self.name = engine.name
        self.calls: list[dict] = []

    def analyse(self, board, depth, multipv, time_cap):
        raw = self.engine._engine
        original = raw.analyse
        nodes = 0

        def observe(*args, **kwargs):
            nonlocal nodes
            infos = original(*args, **kwargs)
            nodes += max((info.get("nodes", 0) for info in infos), default=0)
            return infos

        raw.analyse = observe
        start = time.perf_counter()
        try:
            lines = self.engine.analyse(board, depth, multipv, time_cap=time_cap)
        finally:
            raw.analyse = original
        self.calls.append({
            "requested_depth": depth, "multipv": multipv, "nodes": nodes,
            "elapsed_ms": (time.perf_counter() - start) * 1000,
            "depths": [line.depth for line in lines], "terminal": not lines[0].pv,
        })
        return lines


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    fixture = ROOT / "tests" / "fixtures" / "chesscom" / "2026-11.json"
    pgn = json.loads(fixture.read_text(encoding="utf-8"))["games"][1]["pgn"]
    samples = []
    with tempfile.TemporaryDirectory(prefix="plyback-service-cache-") as directory:
        db = Database(Path(directory) / "cache.db")
        try:
            db.migrate()
            store.save_settings(db, {
                "analysis_depth": 16, "analysis_multipv": 2, "max_seconds_per_position": 5.0,
                "threads_per_worker": 1, "hash_mb": 64,
            })
            row = store.game_row_from_pgn(pgn)
            with db.tx() as connection:
                ((game_id, _),) = store.upsert_games(connection, [row])
            service = AnalysisService(db)
            for label in ("cold", "warm_forced_rerun"):
                engine = Engine(threads=1, hash_mb=64)
                probe = Probe(engine)
                try:
                    assert service.enqueue([game_id], depth=16, force=True) == 1
                    job = service._claim()
                    start = time.perf_counter()
                    service._run_job(job, probe, 0, threading.Event())
                    wall_ms = (time.perf_counter() - start) * 1000
                    status = db.one("SELECT status,error FROM jobs WHERE id=?", (job["id"],))
                    if status["status"] != "done":
                        raise RuntimeError(status["error"] or "Job did not complete")
                    sample = {
                        "scenario": label, "wall_ms": round(wall_ms, 2), "engine_evaluations": len(probe.calls),
                        "uci_searches": sum(not call["terminal"] for call in probe.calls),
                        "nodes": sum(call["nodes"] for call in probe.calls),
                        "engine_ms": round(sum(call["elapsed_ms"] for call in probe.calls), 2),
                        "main_depths_nonterminal": sorted({
                            depth for call in probe.calls if call["requested_depth"] == 16 and not call["terminal"]
                            for depth in call["depths"]
                        }),
                        "threat_searches": sum(call["requested_depth"] == 10 for call in probe.calls),
                        "cached_positions": db.scalar("SELECT COUNT(*) FROM position_cache"),
                    }
                    samples.append(sample)
                    print(json.dumps(sample), flush=True)
                finally:
                    engine.close()
            skipped = service.enqueue([game_id], depth=16) == 0
        finally:
            db.close_all()
    result = {
        "python": platform.python_version(), "platform": platform.system(), "engine": engine.name,
        "plies": row["plies"], "depth": 16, "multipv": 2, "time_cap_seconds": 5,
        "threads": 1, "hash_mb": 64, "fresh_process_each_run": True,
        "normal_queue_skips_completed_analysis": skipped, "samples": samples,
    }
    print(json.dumps(result, indent=2))
    if args.out:
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
