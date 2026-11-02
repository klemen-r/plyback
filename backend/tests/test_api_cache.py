"""Application cache ownership and shutdown are independent of account IDs."""

import threading
from concurrent.futures import ThreadPoolExecutor

from fastapi.testclient import TestClient

from plyback.api.insights import InsightCache
from plyback.main import create_app
from plyback.services import store


def test_apps_do_not_share_insight_payloads(tmp_path):
    apps = [create_app(tmp_path / f"{i}.db", start_background=False, serve_frontend=False) for i in range(2)]
    for app, username in zip(apps, ["alice", "bob"]):
        app.state.services.db.execute("INSERT INTO accounts (id, username, added_at) VALUES (1, ?, 1)", (username,))
    db = apps[0].state.services.db
    row = store.game_row_from_pgn('[White "alice"]\n[Black "opponent"]\n[Result "1-0"]\n\n1. e4 e5 1-0')
    with db.tx() as connection:
        gid = store.upsert_games(connection, [row])[0][0]
        store.link_account_games(connection, 1, "alice", [row], [gid])
    with TestClient(apps[0]) as one, TestClient(apps[1]) as two:
        assert one.get("/api/accounts/1/insights").json()["data_quality"]["games"] == 1
        assert two.get("/api/accounts/1/insights").json()["data_quality"]["games"] == 0


def test_simultaneous_cold_requests_share_one_computation():
    started, release = threading.Event(), threading.Event()
    calls = []
    cache = InsightCache()

    def compute():
        calls.append(1)
        started.set()
        assert release.wait(5)
        return {"value": 42}

    try:
        with ThreadPoolExecutor(max_workers=8) as pool:
            futures = [pool.submit(cache.get, "same", 1, compute) for _ in range(8)]
            assert started.wait(5)
            release.set()
            assert all(f.result()["value"] == 42 for f in futures)
        assert len(calls) == 1
    finally:
        cache.close()


def test_cache_refresh_is_joined_at_shutdown():
    started, release, closed = threading.Event(), threading.Event(), threading.Event()
    cleanup = []
    cache = InsightCache(cleanup=lambda: cleanup.append(1))
    cache.get("key", 1, lambda: {"value": 1})

    def refresh():
        started.set()
        assert release.wait(5)
        return {"value": 2}

    stale = cache.get("key", 2, refresh)
    assert stale["stale"] and stale["value"] == 1
    assert started.wait(5)
    thread = threading.Thread(target=lambda: (cache.close(), closed.set()))
    thread.start()
    assert not closed.wait(0.05)
    release.set()
    thread.join(5)
    assert closed.is_set()
    assert len(cleanup) == 2
