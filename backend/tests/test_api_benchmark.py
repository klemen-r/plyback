"""The optional benchmark must never write through its snapshot/output paths."""
import importlib.util
import sqlite3
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "benchmark_api", Path(__file__).resolve().parents[2] / "scripts/benchmark_api.py"
)
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


def test_snapshot_copy_is_independent_and_creates_no_source_sidecars(tmp_path):
    source = tmp_path / "snapshot.db"
    with sqlite3.connect(source) as db:
        db.execute("CREATE TABLE sample(value TEXT)")
        db.execute("INSERT INTO sample VALUES ('preserved')")
    before = source.read_bytes()
    target = tmp_path / "copy.db"
    benchmark.snapshot_copy(source, target)
    with sqlite3.connect(target) as db:
        db.execute("DELETE FROM sample")
    assert source.read_bytes() == before
    assert sorted(p.name for p in tmp_path.iterdir()) == ["copy.db", "snapshot.db"]


@pytest.mark.parametrize("suffix", ["-wal", "-shm", "-journal"])
def test_active_database_is_rejected(tmp_path, suffix):
    source = tmp_path / "snapshot.db"
    source.touch()
    Path(str(source) + suffix).touch()
    with pytest.raises(ValueError, match="self-contained"):
        benchmark.snapshot_copy(source, tmp_path / "copy.db")
    assert not (tmp_path / "copy.db").exists()


def test_existing_output_cannot_be_overwritten(tmp_path):
    output = tmp_path / "snapshot.db"
    output.write_bytes(b"original database")
    with pytest.raises(FileExistsError):
        benchmark.write_result(output, {"timing": 1})
    assert output.read_bytes() == b"original database"
