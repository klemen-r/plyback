"""Benchmarks take consistent snapshots without changing the user's data."""

import sqlite3
from argparse import Namespace

import pytest

from bench.run import run, scale_db
from plyback import security

from .test_insights import seed


def test_snapshot_includes_wal_and_preserves_shared_games(db, tmp_path):
    db.conn().execute("PRAGMA wal_autocheckpoint=0")
    account = seed(db)
    with db.tx() as connection:
        other = connection.execute("INSERT INTO accounts (username, added_at) VALUES ('other', 1)").lastrowid
        game = connection.execute("SELECT MIN(id) FROM games").fetchone()[0]
        connection.execute("INSERT INTO account_games VALUES (?, ?, 'black')", (other, game))
    target = tmp_path / "scaled.db"
    scale_db(tmp_path / "test.db", target, 2)
    with sqlite3.connect(target) as snapshot:
        assert snapshot.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert snapshot.execute("SELECT COUNT(*) FROM games").fetchone()[0] == 8
        assert snapshot.execute("SELECT COUNT(*) FROM account_games WHERE account_id=?", (account,)).fetchone()[0] == 8
        assert snapshot.execute("SELECT COUNT(*) FROM account_games WHERE account_id=?", (other,)).fetchone()[0] == 2
    assert db.scalar("SELECT COUNT(*) FROM games") == 4
    assert db.scalar("SELECT COUNT(*) FROM account_games") == 5


def test_benchmark_refuses_to_overwrite_source(db, tmp_path):
    source = tmp_path / "test.db"
    with pytest.raises(FileExistsError):
        scale_db(source, source, 1)
    with pytest.raises(ValueError):
        scale_db(source, tmp_path / "invalid.db", 0)


def test_benchmark_runs_against_protected_snapshot(db, tmp_path, monkeypatch):
    seed(db)
    security.set_password(db, "benchmark-test-password")
    target = tmp_path / "benchmark"
    target.mkdir()
    monkeypatch.setenv("PLYBACK_DATA", str(target))
    monkeypatch.setenv("CHESS_INSIGHT_DATA", str(target))
    result = run(Namespace(scale=1, repeat=1, profile=None), tmp_path / "test.db", target)
    assert result["games"] == 4
    assert result["moves"] == db.scalar("SELECT COUNT(*) FROM moves")
    assert result["api_games_page_ms"]["median_ms"] > 0
    assert security.auth_required(db)
    assert db.scalar("SELECT COUNT(*) FROM sessions") == 0
