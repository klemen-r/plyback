import sqlite3
import threading

import pytest

from plyback import db as persistence
from plyback.db import Database


def test_schema_failure_rolls_back_and_can_retry(tmp_path, monkeypatch):
    db = Database(tmp_path / "migration.db")
    monkeypatch.setattr(persistence, "MIGRATIONS", ["CREATE TABLE sample (id INTEGER); SELECT missing FROM sample;"])
    try:
        with pytest.raises(sqlite3.OperationalError):
            db.migrate()
        assert db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='sample'") == 0
        assert not db.conn().in_transaction
        monkeypatch.setattr(persistence, "MIGRATIONS", ["CREATE TABLE sample (id INTEGER);"])
        db.migrate()
        assert db.scalar("SELECT version FROM schema_version") == 1
    finally:
        db.close_all()


def test_backfill_failure_preserves_existing_data_and_schema_version(tmp_path, monkeypatch):
    db = Database(tmp_path / "backfill.db")
    monkeypatch.setattr(persistence, "MIGRATIONS", ["CREATE TABLE sample (id INTEGER);"])
    monkeypatch.setattr(persistence, "DATA_MIGRATIONS", {})
    try:
        db.migrate()
        db.execute("INSERT INTO sample VALUES (7)")

        def broken_backfill(c):
            c.execute("UPDATE sample SET id=99")
            raise RuntimeError("backfill interrupted")

        monkeypatch.setattr(persistence, "MIGRATIONS", ["CREATE TABLE sample (id INTEGER);", "ALTER TABLE sample ADD COLUMN label TEXT;"])
        monkeypatch.setattr(persistence, "DATA_MIGRATIONS", {2: broken_backfill})
        with pytest.raises(RuntimeError, match="backfill interrupted"):
            db.migrate()
        assert db.scalar("SELECT id FROM sample") == 7
        assert db.scalar("SELECT version FROM schema_version") == 1
        assert [r["name"] for r in db.query("PRAGMA table_info(sample)")] == ["id"]
        monkeypatch.setattr(persistence, "DATA_MIGRATIONS", {})
        db.migrate()
        assert db.scalar("SELECT version FROM schema_version") == 2
    finally:
        db.close_all()


def test_newer_database_is_rejected_without_changes(db):
    db.execute("UPDATE schema_version SET version=?", (len(persistence.MIGRATIONS) + 1,))
    with pytest.raises(RuntimeError, match="newer version"):
        db.migrate()
    assert db.scalar("SELECT version FROM schema_version") == len(persistence.MIGRATIONS) + 1


def test_finished_thread_releases_connection_and_can_reopen(db):
    count = len(db._all)
    values = []

    def use_database():
        values.append(db.scalar("SELECT COUNT(*) FROM schema_version"))
        db.close_thread()
        values.append(db.scalar("SELECT COUNT(*) FROM schema_version"))
        db.close_thread()

    worker = threading.Thread(target=use_database)
    worker.start()
    worker.join(5)
    assert not worker.is_alive()
    assert values == [1, 1]
    assert len(db._all) == count


def test_failed_commit_rolls_back_and_leaves_connection_usable(db):
    with pytest.raises(sqlite3.IntegrityError), db.tx() as c:
        c.execute("PRAGMA defer_foreign_keys=ON")
        c.execute("INSERT INTO account_games (account_id, game_id, color) VALUES (999,999,'white')")
    assert not db.conn().in_transaction
    assert db.scalar("SELECT COUNT(*) FROM account_games") == 0
    db.execute("INSERT INTO settings (key, value) VALUES ('after_failure', 'true')")
    assert db.scalar("SELECT value FROM settings WHERE key='after_failure'") == "true"


def test_data_revision_follows_committed_changes_and_rolls_back(db):
    before = db.scalar("SELECT revision FROM data_revision WHERE id=1")
    db.execute("INSERT INTO accounts (username, added_at) VALUES ('sample', 0)")
    inserted = db.scalar("SELECT revision FROM data_revision WHERE id=1")
    assert inserted == before + 1
    db.execute("UPDATE accounts SET display_name='Sample' WHERE username='sample'")
    updated = db.scalar("SELECT revision FROM data_revision WHERE id=1")
    assert updated == inserted + 1
    with pytest.raises(RuntimeError), db.tx() as c:
        c.execute("DELETE FROM accounts WHERE username='sample'")
        raise RuntimeError("interrupted")
    assert db.scalar("SELECT revision FROM data_revision WHERE id=1") == updated
    assert db.scalar("SELECT COUNT(*) FROM accounts") == 1
    db.execute("DELETE FROM accounts WHERE username='sample'")
    assert db.scalar("SELECT revision FROM data_revision WHERE id=1") == updated + 1


def test_data_revision_triggers_cover_all_cache_sources(db):
    triggers = db.query("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'revision_%'")
    assert {r["name"] for r in triggers} == {
        f"revision_{table}_{action}"
        for table in ("games", "account_games", "game_analysis", "accounts")
        for action in ("insert", "update", "delete")
    }
