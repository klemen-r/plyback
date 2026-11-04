"""Regenerate the Rust insight oracle from the preserved Python baseline.

Run with the repository development venv. Uses only committed Chess.com fixtures,
FakeEngine, and a temporary SQLite database. Python is not a production dependency.
"""
import json
import os
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "backend"))

with tempfile.TemporaryDirectory(prefix="plyback-insights-oracle-") as directory:
    os.environ["PLYBACK_DATA"] = directory
    from tests.conftest import FakeEngine
    from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
    from plyback.analysis.pgn import parse_pgn
    from plyback.db import Database
    from plyback.insights.compute import compute_insights
    from plyback.services import store

    store.time.time = lambda: 1_790_000_000
    db = Database(Path(directory) / "fixture.db")
    db.migrate()
    with db.tx() as connection:
        account = connection.execute(
            "INSERT INTO accounts(username,added_at,sync_enabled) VALUES ('klemen_r',1790000000,0)"
        ).lastrowid
    engine = FakeEngine()
    for filename in ("2026-10.json", "2026-11.json"):
        records = json.loads((ROOT / "backend/tests/fixtures/chesscom" / filename).read_text())["games"]
        for record in records:
            row = store.game_row_from_chesscom(record)
            with db.tx() as connection:
                [(game_id, _)] = store.upsert_games(connection, [row])
                store.link_account_games(connection, account, "klemen_r", [row], [game_id])
            analysis = analyze_parsed(
                parse_pgn(record["pgn"]), lambda board, depth, multipv: engine.analyse(board, depth, multipv),
                AnalysisOptions(depth=8),
            )
            store.save_analysis(db, game_id, analysis, 8, 2, engine.name)
    db.execute("UPDATE moves SET endgame_type='rook' WHERE phase='endgame' AND endgame_type IS NULL")
    db.execute("UPDATE moves SET tags=? WHERE game_id=1 AND ply=7", (
        '{"tactical":true,"missed_mate":{"mate":2},"time_pressure":true}',
    ))
    filters = [{}, {"color": "black"}, {"time_classes": "blitz", "color": "white"}, {"until": 0},
               {"rated": "false"}, {"since": 0, "until": 253402300799, "opponent": "klemen_r"}]
    expected = [{"filters": f, "result": compute_insights(db, account, f)} for f in filters]
    # Insight queries never read PGN, PVs, reply lines, cached evaluations or counts.
    # Omit these large inputs while retaining every result-relevant source value.
    db.execute("UPDATE games SET pgn='[Event \"Insight oracle\"]\n\n*'")
    db.execute("UPDATE moves SET lines_json='[]',reply_json=NULL")
    db.execute("UPDATE game_analysis SET key_moments_json='[]',evals_json='[]',counts_json='{}'")
    tables = ["accounts", "games", "account_games", "game_analysis", "moves"]
    statements = list(db.conn().iterdump())
    sql = []
    for table in tables:
        prefix = f'INSERT INTO "{table}"'
        columns = ",".join(
            '"' + row[1].replace('"', '""') + '"'
            for row in db.conn().execute(f'PRAGMA table_info("{table}")')
        )
        # Preserve the baseline's columns when newer native migrations add fields.
        sql.extend(
            line.replace(prefix, f"{prefix}({columns})", 1)
            for line in statements if line.startswith(prefix)
        )
    destination = Path(__file__).parent
    (destination / "insights.sql").write_text("\n".join(sql) + "\n", encoding="utf-8")
    (destination / "insights-expected.json").write_text(json.dumps(expected, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    db.close_all()
