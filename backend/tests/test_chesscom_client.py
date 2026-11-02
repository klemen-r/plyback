import httpx
import pytest

from plyback.chesscom.client import ChessComClient, ChessComError, PlayerNotFound


def make(handler, **kw):
    sleeps = []
    c = ChessComClient(transport=httpx.MockTransport(handler), min_interval=0, sleep=sleeps.append, **kw)
    return c, sleeps


def test_retries_on_429_and_honours_retry_after():
    calls = {"n": 0}

    def handler(req):
        calls["n"] += 1
        if calls["n"] < 3:
            return httpx.Response(429, headers={"Retry-After": "7"})
        return httpx.Response(200, json={"ok": True}, headers={"ETag": '"x"'})

    c, sleeps = make(handler)
    r = c.get("player/foo")
    assert r.data == {"ok": True} and r.etag == '"x"'
    assert calls["n"] == 3
    assert sleeps[0] >= 7


def test_conditional_request_returns_304():
    def handler(req):
        assert req.headers["If-None-Match"] == '"abc"'
        return httpx.Response(304)

    c, _ = make(handler)
    r = c.get("player/foo/games/2026/01", etag='"abc"')
    assert r.not_modified


def test_not_found():
    c, _ = make(lambda req: httpx.Response(404, json={}))
    with pytest.raises(PlayerNotFound):
        c.profile("nobody_here")


def test_gives_up_after_max_retries():
    c, sleeps = make(lambda req: httpx.Response(503), max_retries=2)
    with pytest.raises(ChessComError):
        c.get("player/foo")
    assert len(sleeps) == 2


def test_network_errors_are_retried():
    calls = {"n": 0}

    def handler(req):
        calls["n"] += 1
        if calls["n"] == 1:
            raise httpx.ConnectError("boom")
        return httpx.Response(200, json={"a": 1})

    c, _ = make(handler)
    assert c.get("x").data == {"a": 1}


def test_user_agent_contains_contact():
    seen = {}

    def handler(req):
        seen["ua"] = req.headers["User-Agent"]
        return httpx.Response(200, json={})

    c, _ = make(handler, contact="me@example.com")
    c.get("x")
    assert "Plyback" in seen["ua"] and "me@example.com" in seen["ua"]
