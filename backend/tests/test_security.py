"""Security behaviour: authentication, CSRF, host checks, headers, limits."""

import pytest
from fastapi.testclient import TestClient

from plyback import config, security
from plyback.main import create_app

from .conftest import FakeEngine, fake_engine_factory

H = {"X-Requested-With": "plyback"}


@pytest.fixture
def app(tmp_path, fake_chesscom):
    return create_app(
        tmp_path / "sec.db",
        start_background=False,
        chesscom_client=fake_chesscom.client(),
        engine_factory=fake_engine_factory,
        interactive_factory=lambda: FakeEngine(),
        serve_frontend=False,
    )


@pytest.fixture
def client(app):
    with TestClient(app) as c:
        yield c


def test_password_hashing():
    h = security.hash_password("correct horse")
    assert h.startswith("scrypt$") and "correct" not in h
    assert security.verify_password("correct horse", h)
    assert not security.verify_password("wrong", h)
    assert not security.verify_password("x", "garbage")
    assert security.hash_password("same") != security.hash_password("same")  # salted


def test_security_headers(client):
    r = client.get("/api/health")
    assert r.headers["X-Content-Type-Options"] == "nosniff"
    assert r.headers["X-Frame-Options"] == "DENY"
    assert "frame-ancestors 'none'" in r.headers["Content-Security-Policy"]
    assert "script-src 'self'" in r.headers["Content-Security-Policy"]
    assert r.headers["Cache-Control"] == "no-store"


def test_csrf_requires_custom_header_and_same_origin(client):
    assert client.post("/api/queue/pause").status_code == 403
    assert client.post("/api/queue/pause", headers={**H, "Origin": "https://evil.example"}).status_code == 403
    assert client.post("/api/queue/pause", headers={**H, "Origin": "http://testserver"}).status_code == 200
    assert client.post("/api/queue/pause", headers=H).status_code == 200


def test_host_allowlist_blocks_dns_rebinding(client):
    assert client.get("/api/health", headers={"Host": "attacker.example"}).status_code == 400
    assert client.get("/api/health", headers={"Host": "localhost:8000"}).status_code == 200
    assert client.get("/api/health", headers={"Host": "127.0.0.1:8000"}).status_code == 200


def test_password_flow(client):
    s = client.get("/api/auth/session").json()
    assert s == {"authenticated": True, "password_set": False, "auth_required": False}
    assert client.post("/api/auth/password", json={"password": "short"}, headers=H).status_code == 400
    r = client.post("/api/auth/password", json={"password": "a-strong-password"}, headers=H)
    assert r.status_code == 200 and security.SESSION_COOKIE in r.cookies
    cookie = r.headers["set-cookie"].lower()
    assert "httponly" in cookie and "samesite=strict" in cookie
    assert client.get("/api/accounts").status_code == 200  # signed in by setting the password
    client.post("/api/auth/logout", headers=H)
    client.cookies.clear()
    assert client.get("/api/accounts").status_code == 401
    assert client.get("/api/health").status_code == 200  # public
    assert client.get("/api/auth/session").json()["authenticated"] is False
    assert client.post("/api/auth/login", json={"password": "nope-nope"}, headers=H).status_code == 401
    assert client.post("/api/auth/login", json={"password": "a-strong-password"}, headers=H).status_code == 200
    assert client.get("/api/accounts").status_code == 200
    # Changing the password requires the current one and signs out other sessions.
    assert (
        client.post("/api/auth/password", json={"current": "wrong", "password": "another-password"}, headers=H).status_code
        == 403
    )
    assert (
        client.post(
            "/api/auth/password", json={"current": "a-strong-password", "password": "another-password"}, headers=H
        ).status_code
        == 200
    )


def test_with_password_any_host_is_allowed_but_needs_session(client):
    client.post("/api/auth/password", json={"password": "a-strong-password"}, headers=H)
    client.cookies.clear()
    r = client.get("/api/accounts", headers={"Host": "chess.example.net"})
    assert r.status_code == 401


def test_session_tokens_are_stored_hashed(app, client):
    r = client.post("/api/auth/password", json={"password": "a-strong-password"}, headers=H)
    token = r.cookies[security.SESSION_COOKIE]
    rows = app.state.services.db.query("SELECT token_hash FROM sessions")
    assert rows and all(row["token_hash"] != token for row in rows)


def test_password_hash_not_exposed_or_settable(client):
    client.post("/api/auth/password", json={"password": "a-strong-password"}, headers=H)
    assert "auth_password_hash" not in client.get("/api/settings").text
    assert client.put("/api/settings", json={"auth_password_hash": "x"}, headers=H).status_code == 400


def test_login_rate_limited(client):
    client.post("/api/auth/password", json={"password": "a-strong-password"}, headers=H)
    client.cookies.clear()
    codes = [client.post("/api/auth/login", json={"password": "guess-guess"}, headers=H).status_code for _ in range(12)]
    assert 429 in codes


def test_body_size_limit(client):
    r = client.post(
        "/api/games/import",
        content=b"x",
        headers={**H, "Content-Length": str(config.MAX_BODY_BYTES + 1), "Content-Type": "application/json"},
    )
    assert r.status_code == 413


def test_unknown_api_route_is_json_404(client):
    r = client.get("/api/does-not-exist")
    assert r.status_code == 404 and r.json()["detail"] == "Not found"


def test_input_validation(client):
    assert client.post("/api/accounts", json={"username": "../etc/passwd"}, headers=H).status_code == 400
    assert client.post("/api/engine/analyse", json={"fen": "x" * 5000}, headers=H).status_code in (400, 422)
    assert client.post("/api/accounts/1/analyze", json={"depth": -3}, headers=H).status_code in (404, 422)


def test_refuses_unprotected_network_binding(monkeypatch, tmp_path):
    from plyback import main as main_mod

    monkeypatch.setattr(config, "HOST", "0.0.0.0")
    monkeypatch.setattr(config, "DB_PATH", tmp_path / "x.db")
    monkeypatch.setattr(config, "LEGACY_DB_PATH", tmp_path / "legacy.db")
    monkeypatch.setattr(config, "INITIAL_PASSWORD", None)
    monkeypatch.setattr(config, "ALLOW_UNAUTHENTICATED_REMOTE", False)
    with pytest.raises(SystemExit) as exc:
        main_mod.run()
    assert "password" in str(exc.value)


def test_archive_urls_outside_api_are_ignored(db, fake_chesscom):
    from plyback.services.analysis import AnalysisService
    from plyback.services.sync import SyncService

    original = fake_chesscom.handler

    def evil(request):
        if request.url.path.endswith("/games/archives"):
            import httpx

            return httpx.Response(200, json={"archives": ["http://169.254.169.254/latest/meta-data/2026/11"]})
        return original(request)

    fake_chesscom.handler = evil
    import httpx

    from plyback.chesscom.client import ChessComClient

    client = ChessComClient(transport=httpx.MockTransport(evil), min_interval=0, sleep=lambda s: None)
    sync = SyncService(db, AnalysisService(db, engine_factory=fake_engine_factory), client=client)
    with db.tx() as c:
        acc = c.execute("INSERT INTO accounts (username, added_at) VALUES ('klemen_r', 0)").lastrowid
    sync.sync_account(acc)
    assert db.scalar("SELECT COUNT(*) FROM games") == 0
    assert not any("169.254" in call for call in fake_chesscom.calls)
