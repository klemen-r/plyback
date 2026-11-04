"""Create an isolated synthetic archive from committed fixtures, without real engine searches.

Usage: python scripts/seed_benchmark.py /new/path/archive.db --scale 100
Never overwrites a database. The fake scores measure application overhead, not chess quality.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "backend"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--scale", type=int, default=1)
    args = parser.parse_args()
    if args.scale < 1 or args.output.exists():
        parser.error("scale must be positive and the output must not already exist")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="plyback-fixture-seed-") as directory:
        os.environ["PLYBACK_DATA"] = os.environ["CHESS_INSIGHT_DATA"] = directory
        from bench.run import scale_db
        from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
        from plyback.analysis.pgn import parse_pgn
        from plyback.db import Database
        from plyback.services import store
        from tests.conftest import FakeEngine

        source = Path(directory) / "seed.db"
        db = Database(source)
        try:
            db.migrate()
            with db.tx() as tx:
                account = tx.execute(
                    "INSERT INTO accounts(username,added_at,sync_enabled) VALUES ('klemen_r',1790000000,0)"
                ).lastrowid
            engine = FakeEngine()
            for name in ("2026-10.json", "2026-11.json"):
                fixture = ROOT / "backend/tests/fixtures/chesscom" / name
                for game in json.loads(fixture.read_text(encoding="utf-8"))["games"]:
                    row = store.game_row_from_chesscom(game)
                    with db.tx() as tx:
                        [(gid, _)] = store.upsert_games(tx, [row])
                        store.link_account_games(tx, account, "klemen_r", [row], [gid])
                    result = analyze_parsed(
                        parse_pgn(game["pgn"]),
                        lambda board, depth, multipv: engine.analyse(board, depth, multipv),
                        AnalysisOptions(depth=8),
                    )
                    store.save_analysis(db, gid, result, 8, 2, engine.name)
        finally:
            db.close_all()
        scale_db(source, args.output, args.scale)
    print(f"Created fixture-only archive: {args.output} (scale {args.scale})")


if __name__ == "__main__":
    main()
