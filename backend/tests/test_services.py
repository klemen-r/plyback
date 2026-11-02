"""Sync + analysis queue + reports, end to end with a fake engine and captured API data."""

import threading
import time
from datetime import datetime, timezone

from plyback.db import dumps
from plyback.services import store
from plyback.services.analysis import PRIORITY_HISTORY, PRIORITY_NEW, AnalysisService, _position_key
from plyback.services.reports import ReportService
from plyback.services.sync import SyncService, _month_is_closed

from .conftest import FakeEngine, fake_engine_factory, load_fixture


def add_account(db, username="klemen_r", **kw):
    with db.tx() as c:
        cur = c.execute(
            "INSERT INTO accounts (username, added_at, analyze_history, auto_analyze) VALUES (?,?,?,?)",
            (username, int(time.time()), kw.get("analyze_history", 1), kw.get("auto_analyze", 1)),
        )
        return cur.lastrowid


def wait_for(cond, timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        if cond():
            return True
        time.sleep(0.1)
    return False


def test_initial_sync_imports_and_queues_history(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db)
    summary = sync.sync_account(acc)
    assert summary["new_games"] == 11
    assert db.scalar("SELECT COUNT(*) FROM games") == 11
    assert db.scalar("SELECT COUNT(*) FROM account_games WHERE account_id=?", (acc,)) == 11
    # colors are linked correctly
    colors = {r["color"] for r in db.query("SELECT color FROM account_games")}
    assert colors <= {"white", "black"}
    # openings detected at import time (engine-free)
    assert db.scalar("SELECT COUNT(*) FROM games WHERE opening_family IS NOT NULL") == 11
    jobs = db.query("SELECT priority FROM jobs WHERE status='queued'")
    assert len(jobs) == 11 and all(j["priority"] == PRIORITY_HISTORY for j in jobs)
    a = store.account_by_id(db, acc)
    assert a["sync_status"] == "idle" and a["last_sync_at"] and a["avatar"]
    # past month is complete, current month is not
    arch = {r["period"]: r for r in db.query("SELECT * FROM archives")}
    expected = 1 if _month_is_closed("2026/10", datetime.now(timezone.utc)) else 0
    assert arch["2026/10"]["complete"] == expected
    assert arch["2026/11"]["etag"]


def test_incremental_sync_detects_new_game(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db, analyze_history=0)
    sync.sync_account(acc)
    assert db.scalar("SELECT COUNT(*) FROM jobs") == 0  # history not auto-analysed
    calls_before = len(fake_chesscom.calls)
    sync.sync_account(acc)  # nothing new → 304s
    assert db.scalar("SELECT COUNT(*) FROM games") == 11
    if _month_is_closed("2026/10", datetime.now(timezone.utc)):  # complete months are never re-fetched
        assert not any(c.endswith("/2026/10") for c in fake_chesscom.calls[calls_before:])
    fake_chesscom.extra_games = load_fixture("2026-11-extra.json")["games"]
    s = sync.sync_account(acc)
    assert s["new_games"] == 1
    job = db.one("SELECT * FROM jobs")
    assert job["priority"] == PRIORITY_NEW
    assert store.account_by_id(db, acc)["last_new_game_at"]


def test_sync_handles_missing_player_and_outage(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db, "ghost_user")
    fake_chesscom.missing_users.add("ghost_user")
    sync.sync_account(acc)
    a = store.account_by_id(db, acc)
    assert a["sync_status"] == "error" and "not found" in a["sync_error"]
    acc2 = add_account(db, "klemen_r")
    fake_chesscom.fail_next = [503] * 12  # profile refresh (tolerated) + archives (fatal)
    sync.sync_account(acc2)
    assert store.account_by_id(db, acc2)["sync_status"] == "error"
    sync.sync_account(acc2)  # recovers on the next attempt
    assert store.account_by_id(db, acc2)["sync_status"] == "idle"


def test_analysis_queue_processes_and_stores(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    store.save_settings(db, {"workers": 2, "analysis_depth": 8})
    acc = add_account(db)
    sync.sync_account(acc)
    analysis.start()
    try:
        assert wait_for(lambda: db.scalar("SELECT COUNT(*) FROM game_analysis") == 11, 120)
    finally:
        analysis.stop()
    g = db.one("SELECT * FROM game_analysis LIMIT 1")
    assert g["engine"] == "FakeFish 1.0" and g["accuracy_white"] is not None
    plies = db.scalar("SELECT plies FROM games WHERE id=?", (g["game_id"],))
    assert db.scalar("SELECT COUNT(*) FROM moves WHERE game_id=?", (g["game_id"],)) == plies
    assert db.scalar("SELECT COUNT(*) FROM jobs WHERE status IN ('queued','running')") == 0
    assert db.scalar("SELECT COUNT(*) FROM position_cache") > 0
    # Re-queueing at the same depth is a no-op (analysis is reused).
    assert analysis.enqueue([g["game_id"]], depth=8) == 0
    assert analysis.enqueue([g["game_id"]], depth=12) == 1


def test_interrupted_job_resumes_from_checkpoint(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db)
    sync.sync_account(acc)
    game_id = db.scalar("SELECT game_id FROM jobs ORDER BY sort_key DESC LIMIT 1")
    # Simulate a crash mid-analysis: job left 'running' with a partial checkpoint.
    from plyback.analysis.pgn import parse_pgn

    parsed = parse_pgn(db.scalar("SELECT pgn FROM games WHERE id=?", (game_id,)))
    eng = FakeEngine()
    partial = {}
    for b in parsed.boards[:10]:
        key = _position_key(b, 2, eng.name, 5.0)
        partial[key] = {"depth": 16, "lines": [ln.to_dict() for ln in eng.analyse(b, 16, 2)]}
    with db.tx() as c:
        c.execute("UPDATE jobs SET status='running', partial_json=? WHERE game_id=?", (dumps(partial), game_id))
    counting = FakeEngine()
    analysis2 = AnalysisService(db, engine_factory=lambda t, h: counting)
    analysis2.start()  # resets running → queued
    try:
        assert wait_for(lambda: db.scalar("SELECT COUNT(*) FROM game_analysis WHERE game_id=?", (game_id,)) == 1)
    finally:
        analysis2.stop()
    assert counting.calls > 0


def test_failing_engine_marks_job_failed_after_retries(db, fake_chesscom):
    class Broken(FakeEngine):
        def analyse(self, *a, **k):
            raise RuntimeError("engine exploded")

    analysis = AnalysisService(db, engine_factory=lambda t, h: Broken())
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db)
    store.save_settings(db, {"workers": 1})
    sync.sync_account(acc)
    with db.tx() as c:
        c.execute("DELETE FROM jobs WHERE id NOT IN (SELECT MIN(id) FROM jobs)")
    analysis.start()
    try:
        assert wait_for(lambda: db.scalar("SELECT COUNT(*) FROM jobs WHERE status='failed'") == 1)
    finally:
        analysis.stop()
    job = db.one("SELECT * FROM jobs")
    assert job["attempts"] == 3 and "exploded" in job["error"]
    assert analysis.retry_failed() == 1


def test_report_lifecycle(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    reports = ReportService(db, analysis)
    acc = add_account(db, analyze_history=0)
    sync.sync_account(acc)
    rid = reports.create(acc, {"time_classes": ["blitz"]}, "Blitz report")
    rep = db.one("SELECT * FROM reports WHERE id=?", (rid,))
    assert rep["status"] == "analyzing" and rep["total_games"] > 0
    assert db.scalar("SELECT COUNT(*) FROM jobs WHERE status='queued'") == rep["total_games"]
    analysis.start()
    try:
        assert wait_for(lambda: db.scalar("SELECT COUNT(*) FROM jobs WHERE status IN ('queued','running')") == 0, 120)
    finally:
        analysis.stop()
    reports.tick()
    rep = db.one("SELECT * FROM reports WHERE id=?", (rid,))
    assert rep["status"] == "ready" and rep["analyzed_games"] == rep["total_games"]
    assert '"priorities"' in rep["report_json"]


def test_month_closing_rule():
    assert _month_is_closed("2026/09", datetime(2026, 10, 3, 1, tzinfo=timezone.utc))
    assert not _month_is_closed("2026/09", datetime(2026, 10, 2, 1, tzinfo=timezone.utc))
    assert _month_is_closed("2025/12", datetime(2026, 1, 5, tzinfo=timezone.utc))


def test_retry_failed_with_duplicate_failed_rows(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    SyncService(db, analysis, client=fake_chesscom.client()).sync_account(add_account(db, analyze_history=0))
    gid = db.scalar("SELECT MIN(id) FROM games")
    with db.tx() as c:
        for _ in range(2):
            c.execute("INSERT INTO jobs (game_id, status, created_at, attempts) VALUES (?, 'failed', 0, 3)", (gid,))
    assert analysis.retry_failed() == 1
    assert db.scalar("SELECT COUNT(*) FROM jobs WHERE game_id=?", (gid,)) == 1


def test_interrupted_first_import_stays_history(db, fake_chesscom):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    sync = SyncService(db, analysis, client=fake_chesscom.client())
    acc = add_account(db, analyze_history=0)
    fake_chesscom.fail_next = [503] * 12  # first sync fails part-way
    sync.sync_account(acc)
    assert not store.account_by_id(db, acc)["history_complete"]
    sync.sync_account(acc)  # completes: games are history, not "new"
    a = store.account_by_id(db, acc)
    assert a["history_complete"] and not a["last_new_game_at"]
    assert db.scalar("SELECT COUNT(*) FROM jobs") == 0


def test_stop_during_threat_tests_is_not_swallowed():
    from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
    from plyback.analysis.pgn import parse_pgn
    from plyback.services.analysis import Interrupted

    eng = FakeEngine()

    def evaluate(b, d, m):
        if d == 10:  # threat-test depth
            raise Interrupted()
        return eng.analyse(b, d, m)

    import pytest

    with pytest.raises(Interrupted):
        analyze_parsed(parse_pgn("1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 *"), evaluate, AnalysisOptions(depth=12))


def test_reconfigured_workers_are_joined_before_shutdown(db, monkeypatch):
    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    store.save_settings(db, {"workers": 1})
    started = [threading.Event(), threading.Event()]
    release = threading.Event()
    stopped = threading.Event()

    def worker(wid, stop):
        started[wid].set()
        if wid == 0:
            release.wait(10)
        else:
            stop.wait(10)

    monkeypatch.setattr(analysis, "_worker_loop", worker)
    analysis.apply_settings()
    stopper = threading.Thread(target=lambda: (analysis.stop(), stopped.set()))
    try:
        assert started[0].wait(5)
        store.save_settings(db, {"hash_mb": 128})
        analysis.apply_settings()
        assert started[1].wait(5)
        assert analysis.stats()["workers"] == 1
        assert len(analysis._workers) == 2  # retiring worker remains tracked
        stopper.start()
        assert not stopped.wait(0.05)
        release.set()
        assert stopped.wait(5)
        assert not analysis._workers and not analysis._worker_stop
    finally:
        release.set()
        analysis.stop()
        if stopper.ident is not None:
            stopper.join(5)


def test_position_cache_distinguishes_draw_history_and_engine_budget():
    import chess

    board = chess.Board()
    for san in ["Nf3", "Nf6", "Ng1", "Ng8"] * 4:
        board.push_san(san)
    fresh = chess.Board(board.fen())
    assert board.epd() == fresh.epd()
    assert board.is_fivefold_repetition() and not fresh.is_fivefold_repetition()
    key = _position_key(board, 2, "Fish 1", 5.0)
    assert key == _position_key(board.copy(), 2, "Fish 1", 5.0)
    assert key != _position_key(fresh, 2, "Fish 1", 5.0)
    assert key != _position_key(board, 2, "Fish 2", 5.0)
    assert key != _position_key(board, 2, "Fish 1", 10.0)
    fresh.halfmove_clock += 1
    assert _position_key(fresh, 2, "Fish 1", 5.0) != _position_key(chess.Board(board.fen()), 2, "Fish 1", 5.0)


def test_analysis_revision_changes_even_with_identical_timestamp(db, monkeypatch):
    from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
    from plyback.analysis.pgn import parse_pgn

    pgn = "1. e4 e5 *"
    with db.tx() as c:
        gid = store.upsert_games(c, [store.game_row_from_pgn(pgn)])[0][0]
    engine = FakeEngine()
    result = analyze_parsed(parse_pgn(pgn), engine.analyse, AnalysisOptions(depth=6))
    monkeypatch.setattr(store.time, "time", lambda: 1234567890)
    store.save_analysis(db, gid, result, 6, 2, engine.name)
    first = db.one("SELECT analyzed_at, revision FROM game_analysis WHERE game_id=?", (gid,))
    store.save_analysis(db, gid, result, 6, 2, engine.name)
    second = db.one("SELECT analyzed_at, revision FROM game_analysis WHERE game_id=?", (gid,))
    assert first["analyzed_at"] == second["analyzed_at"]
    assert second["revision"] == first["revision"] + 1


def test_finalized_report_is_immutable(db, monkeypatch):
    from plyback.services import reports as report_module

    analysis = AnalysisService(db, engine_factory=fake_engine_factory)
    reports = ReportService(db, analysis)
    rid = reports.create(add_account(db), {}, "Snapshot")
    reports.finalize(rid)
    first = db.one("SELECT * FROM reports WHERE id=?", (rid,))
    assert first["status"] == "ready"

    def unexpected_rebuild(*args):
        raise AssertionError("A saved snapshot must not be recomputed")

    monkeypatch.setattr(report_module, "compute_insights", unexpected_rebuild)
    reports.finalize(rid)
    assert db.one("SELECT * FROM reports WHERE id=?", (rid,)) == first
