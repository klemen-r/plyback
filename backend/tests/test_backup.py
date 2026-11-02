"""Backups include committed WAL content and never overwrite data."""

import sqlite3
from contextlib import closing

import pytest

from plyback import config
from plyback.main import adopt_legacy_database, backup_database


def test_backup_includes_live_wal_and_refuses_overwrite(tmp_path):
    source, backup = tmp_path / "source.db", tmp_path / "backup.db"
    with closing(sqlite3.connect(source)) as writer:
        writer.execute("PRAGMA journal_mode=WAL")
        writer.execute("CREATE TABLE evidence (value TEXT)")
        writer.execute("INSERT INTO evidence VALUES ('committed')")
        writer.commit()
        backup_database(source, backup)
        with closing(sqlite3.connect(backup)) as reader:
            assert reader.execute("SELECT value FROM evidence").fetchone()[0] == "committed"
            assert reader.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        before = backup.read_bytes()
        with pytest.raises(FileExistsError):
            backup_database(source, backup)
        assert backup.read_bytes() == before
    assert not list(tmp_path.glob(".plyback-backup-*"))


def test_legacy_adoption_preserves_original(tmp_path, monkeypatch):
    source, target = tmp_path / "chess-insight.db", tmp_path / "plyback.db"
    monkeypatch.setattr(config, "LEGACY_DB_PATH", source)
    monkeypatch.setattr(config, "DB_PATH", target)
    with closing(sqlite3.connect(source)) as writer:
        writer.execute("CREATE TABLE evidence (value INTEGER)")
        writer.execute("INSERT INTO evidence VALUES (7)")
        writer.commit()
    original = source.read_bytes()
    adopt_legacy_database()
    assert source.read_bytes() == original
    with closing(sqlite3.connect(target)) as reader:
        assert reader.execute("SELECT value FROM evidence").fetchone()[0] == 7
    adopted = target.read_bytes()
    adopt_legacy_database()
    assert target.read_bytes() == adopted
