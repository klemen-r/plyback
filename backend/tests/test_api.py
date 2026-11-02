"""HTTP API integration tests (FastAPI TestClient, fake engine, captured Chess.com data)."""

import time

import pytest
from fastapi.testclient import TestClient

from plyback.main import create_app

from .conftest import FakeEngine, fake_engine_factory

LEGAL = """[Event "Casual"]
[White "klemen_r"]
[Black "Friend"]
[Result "1-0"]
[TimeControl "300"]

1. e4 {[%clk 0:04:59]} e5 {[%clk 0:04:58]} 2. Nf3 {[%clk 0:04:55]} d6 {[%clk 0:04:50]}
3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 Bxd1 6. Bxf7+ Ke7 7. Nd5# 1-0
"""


@pytest.fixture
def client(tmp_path, fake_chesscom):
    app = create_app(
        tmp_path / "api.db",
        start_background=True,
        chesscom_client=fake_chesscom.client(),
        engine_factory=fake_engine_factory,
        interactive_factory=lambda: FakeEngine(),
        serve_frontend=False,
    )
    with TestClient(app, headers={"X-Requested-With": "plyback"}) as c:
        c.put("/api/settings", json={"workers": 2, "sync_interval_seconds": 3600})
        yield c


def wait(cond, timeout=90):
    t0 = time.time()
    while time.time() - t0 < timeout:
        if cond():
            return True
        time.sleep(0.2)
    return False


def test_health_and_status(client):
    assert client.get("/api/health").json()["ok"]
    s = client.get("/api/status").json()
    assert "queue" in s and s["accounts"] == 0


def test_settings_validation(client):
    r = client.put("/api/settings", json={"analysis_depth": 99})
    assert r.status_code == 400
    r = client.put("/api/settings", json={"bogus": 1})
    assert r.status_code == 400
    r = client.put("/api/settings", json={"analysis_depth": 14})
    assert r.status_code == 200 and r.json()["analysis_depth"] == 14


def test_account_flow_sync_analyse_review_insights(client, fake_chesscom):
    r = client.post("/api/accounts", json={"username": "klemen_r"})
    assert r.status_code == 201, r.text
    acc = r.json()
    assert acc["username"].lower() == "klemen_r"
    assert client.post("/api/accounts", json={"username": "klemen_r"}).status_code == 409
    assert wait(lambda: client.get(f"/api/accounts/{acc['id']}").json()["games"] == 11)
    assert wait(lambda: client.get(f"/api/accounts/{acc['id']}").json()["analyzed"] == 11, 180)

    games = client.get("/api/games", params={"account_id": acc["id"], "page_size": 5}).json()
    assert games["total"] == 11 and len(games["items"]) == 5
    g = games["items"][0]
    assert g["user_color"] in ("white", "black") and g["outcome"] in ("win", "loss", "draw") and g["analyzed"]
    wins = client.get("/api/games", params={"account_id": acc["id"], "outcome": "win"}).json()
    assert all(x["outcome"] == "win" for x in wins["items"])

    detail = client.get(f"/api/games/{g['id']}", params={"account_id": acc["id"]}).json()
    assert detail["analysis"]["accuracy"]["white"] is not None
    assert len(detail["moves"]) == len(detail["analysis"]["evals"]) - 1
    m = detail["moves"][0]
    assert {"classification", "explanation", "lines", "fen_before", "clock"} <= m.keys()

    ins = client.get(f"/api/accounts/{acc['id']}/insights").json()
    assert ins["overview"]["games"] == 11 and ins["data_quality"]["analyzed"] == 11
    for key in ("openings", "phases", "errors", "time", "advantage", "recurring", "priorities", "ratings", "sessions"):
        assert key in ins
    filtered = client.get(f"/api/accounts/{acc['id']}/insights", params={"color": "white"}).json()
    assert filtered["overview"]["black"]["games"] == 0

    rep = client.post(f"/api/accounts/{acc['id']}/reports", json={"filters": {}}).json()
    assert wait(lambda: client.get(f"/api/reports/{rep['id']}").json()["status"] == "ready", 30)
    full = client.get(f"/api/reports/{rep['id']}").json()
    assert full["report"]["overview"]["games"] == 11

    assert client.delete(f"/api/accounts/{acc['id']}").json()["games_removed"] == 11
    assert client.get("/api/games").json()["total"] == 0


def test_unknown_user_rejected(client, fake_chesscom):
    fake_chesscom.missing_users.add("nobody")
    r = client.post("/api/accounts", json={"username": "nobody"})
    assert r.status_code == 404
    assert client.post("/api/accounts", json={"username": "bad name!"}).status_code == 400


def test_import_pgn_and_manual_analysis(client):
    r = client.post("/api/games/import", json={"pgn": LEGAL + "\n" + LEGAL})
    data = r.json()
    assert data["imported"] == 1 and data["existing"] == 1
    gid = data["game_ids"][0]
    before = client.get(f"/api/games/{gid}").json()
    assert before["moves"][0]["san"] == "e4"
    assert wait(lambda: client.get(f"/api/games/{gid}").json()["analysis"] is not None)
    d = client.get(f"/api/games/{gid}").json()
    assert d["game"]["final_fen"] and d["analysis"]["opening"]["name"]
    assert client.post(f"/api/games/{gid}/analyze", json={"depth": 20, "force": True}).json()["queued"]
    bad = client.post("/api/games/import", json={"pgn": "not a pgn"})
    assert bad.status_code == 400


def test_interactive_engine_and_queue_controls(client):
    r = client.post(
        "/api/engine/analyse", json={"fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1", "depth": 10}
    )
    assert r.status_code == 200 and r.json()["lines"][0]["san"]
    assert client.post("/api/engine/analyse", json={"fen": "garbage"}).status_code == 400
    assert client.post("/api/queue/pause").json()["paused"] is True
    assert client.post("/api/queue/resume").json()["paused"] is False
    assert client.post("/api/queue/nope").status_code == 404
    q = client.get("/api/queue").json()
    assert "jobs" in q and "failed_jobs" in q
