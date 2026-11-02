"""Security regressions for streaming uploads, redirects, origins and bounded API inputs."""

import httpx
import pytest
from fastapi.testclient import TestClient

from plyback import config, security
from plyback.chesscom.client import ChessComClient, ChessComError
from plyback.main import create_app

from .conftest import fake_engine_factory

H = {"X-Requested-With": "plyback"}


@pytest.fixture
def client(tmp_path, monkeypatch):
    monkeypatch.setattr(config, "MAX_BODY_BYTES", 1024)
    app = create_app(tmp_path / "api.db", start_background=False, serve_frontend=False, engine_factory=fake_engine_factory)
    with TestClient(app, headers=H) as client:
        yield client


@pytest.mark.parametrize(
    "origin", ["null", "https://testserver", "http://testserver:8001", "https://evil.example", "http://[", "http://testserver:0"]
)
def test_cross_origin_includes_scheme_and_port(client, origin):
    assert client.post("/api/queue/pause", headers={"Origin": origin}).status_code == 403


def test_same_origin_with_explicit_default_port(client):
    assert client.post("/api/queue/pause", headers={"Origin": "http://testserver:80"}).status_code == 200


def test_streaming_body_cannot_bypass_size_limit(client):
    response = client.post(
        "/api/games/import", content=iter([b'{"pgn":"', b"x" * 2000, b'"}']), headers={"Content-Type": "application/json"}
    )
    assert response.status_code == 413
    assert response.headers["Cache-Control"] == "no-store"
    assert response.headers["X-Content-Type-Options"] == "nosniff"


def test_testserver_is_not_a_production_host_exception(client, monkeypatch):
    monkeypatch.setattr(config, "ALLOWED_HOSTS", [])
    assert client.get("/api/health").status_code == 400
    assert client.get("/api/health", headers={"Host": "[::1]:8000"}).status_code == 200


@pytest.mark.parametrize("path", ["/api/events?limit=-1", "/api/games?page_size=-1", "/api/games?page=0"])
def test_pagination_cannot_disable_limits(client, path):
    assert client.get(path).status_code == 422


def test_password_cannot_be_removed_while_network_bound(client, monkeypatch):
    client.post("/api/auth/password", json={"password": "a-strong-password"})
    monkeypatch.setattr(config, "HOST", "0.0.0.0")
    response = client.post(
        "/api/auth/password", json={"current": "a-strong-password", "password": None}, headers={"Host": "localhost"}
    )
    # Cookies are host scoped; authenticate for the same host before attempting removal.
    if response.status_code == 401:
        client.post("/api/auth/login", json={"password": "a-strong-password"}, headers={"Host": "localhost"})
        response = client.post(
            "/api/auth/password", json={"current": "a-strong-password", "password": None}, headers={"Host": "localhost"}
        )
    assert response.status_code == 400


@pytest.mark.parametrize(
    "target",
    [
        "http://169.254.169.254/latest",
        "https://api.chess.com.evil/pub/player/x",
        "https://api.chess.com/private",
        "https://api.chess.com/pub/%2e%2e/private",
    ],
)
def test_redirect_targets_checked_before_network_access(target):
    calls = []

    def handler(request):
        calls.append(str(request.url))
        return httpx.Response(302, headers={"Location": target})

    client = ChessComClient(transport=httpx.MockTransport(handler), min_interval=0)
    try:
        with pytest.raises(ChessComError, match="outside"):
            client.profile("example")
        assert len(calls) == 1
    finally:
        client.close()


def test_same_api_redirect_remains_supported():
    calls = []

    def handler(request):
        calls.append(str(request.url))
        return (
            httpx.Response(302, headers={"Location": "/pub/player/new"})
            if len(calls) == 1
            else httpx.Response(200, json={"username": "new"})
        )

    client = ChessComClient(transport=httpx.MockTransport(handler), min_interval=0)
    try:
        assert client.profile("old")["username"] == "new"
        assert len(calls) == 2
    finally:
        client.close()


def test_cancelled_client_does_not_make_requests():
    client = ChessComClient(transport=httpx.MockTransport(lambda r: pytest.fail("unexpected request")))
    client.cancel()
    try:
        with pytest.raises(ChessComError, match="cancelled"):
            client.profile("example")
    finally:
        client.close()


def test_ipv6_host_parser():
    assert security._host_of("[::1]:8000") == "::1"


def test_partial_startup_failure_releases_resources(tmp_path, monkeypatch):
    from plyback.services.reports import ReportService

    workers = []

    def fail_start(self):
        workers.extend(self.analysis._workers.values())
        raise RuntimeError("intentional startup failure")

    monkeypatch.setattr(ReportService, "start", fail_start)
    app = create_app(tmp_path / "startup.db", engine_factory=fake_engine_factory, serve_frontend=False)
    services = app.state.services
    with pytest.raises(RuntimeError, match="intentional startup failure"), TestClient(app):
        pass
    assert not services.sync._thread.is_alive()
    assert all(not thread.is_alive() for thread in workers)
    assert not services.db._all
