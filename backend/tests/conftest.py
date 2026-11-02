from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import chess
import httpx
import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
FIXTURES = Path(__file__).parent / "fixtures"


@pytest.fixture(autouse=True)
def allow_test_host(monkeypatch):
    from plyback import config

    monkeypatch.setattr(config, "ALLOWED_HOSTS", ["testserver"])

# Find the installed engine before redirecting the data directory, then never touch the
# user's real data directory from tests.
_engine_path_file = ROOT.parent / "data" / "engine" / "stockfish.path"
if "PLYBACK_STOCKFISH" not in os.environ and _engine_path_file.exists():
    os.environ["PLYBACK_STOCKFISH"] = _engine_path_file.read_text().strip()
os.environ["PLYBACK_DATA"] = str(Path(__file__).parent / ".testdata")

from plyback.analysis import material  # noqa: E402
from plyback.analysis.engine import Line, locate_stockfish, terminal_eval  # noqa: E402
from plyback.chesscom.client import ChessComClient  # noqa: E402
from plyback.db import Database  # noqa: E402


def stockfish_available() -> bool:
    try:
        locate_stockfish()
        return True
    except Exception:
        return False


requires_stockfish = pytest.mark.skipif(not stockfish_available(), reason="Stockfish not installed")


class FakeEngine:
    """Deterministic stand-in for Stockfish: evaluates material and prefers captures.

    Good enough to exercise the services end-to-end quickly; classification
    correctness is tested against the real engine separately."""

    name = "FakeFish 1.0"

    def __init__(self, *args, **kwargs):
        self.calls = 0
        self.closed = False

    def analyse(self, board: chess.Board, depth: int, multipv: int = 1, time_cap=None, nodes=None, cancel=None) -> list[Line]:
        self.calls += 1
        term = terminal_eval(board)
        if term is not None:
            return [Line(term, [], 0)]
        scored = []
        for mv in board.legal_moves:
            b = board.copy(stack=False)
            b.push(mv)
            t = terminal_eval(b)
            cp = t if t is not None else 100 * material.balance(b, chess.WHITE)
            scored.append((cp if board.turn == chess.WHITE else -cp, mv.uci(), cp))
        scored.sort(key=lambda x: (-x[0], x[1]))
        return [Line(cp, [u], depth) for _, u, cp in scored[: max(1, multipv)]]

    def close(self):
        self.closed = True


def fake_engine_factory(threads=1, hash_mb=16):
    return FakeEngine()


def load_fixture(name: str):
    return json.loads((FIXTURES / "chesscom" / name).read_text(encoding="utf-8"))


class FakeChessCom:
    """Serves captured Chess.com responses through an httpx MockTransport."""

    def __init__(self):
        self.calls: list[str] = []
        self.extra_games: list[dict] = []
        self.missing_users: set[str] = set()
        self.fail_next: list[int] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        path = request.url.path
        self.calls.append(path)
        if self.fail_next:
            return httpx.Response(self.fail_next.pop(0), headers={"Retry-After": "0"})
        user = path.split("/player/")[1].split("/")[0] if "/player/" in path else ""
        if user in self.missing_users:
            return httpx.Response(404, json={"message": "not found"})
        if path.endswith("/games/archives"):
            return httpx.Response(200, json=load_fixture("archives.json"))
        if path.endswith("/games/2026/10"):
            if request.headers.get("If-None-Match") == '"oct"':
                return httpx.Response(304)
            return httpx.Response(200, json=load_fixture("2026-10.json"), headers={"ETag": '"oct"'})
        if path.endswith("/games/2026/11"):
            data = load_fixture("2026-11.json")
            data["games"] += self.extra_games
            etag = f'"nov-{len(data["games"])}"'
            if request.headers.get("If-None-Match") == etag:
                return httpx.Response(304)
            return httpx.Response(200, json=data, headers={"ETag": etag})
        if path.endswith("/stats"):
            return httpx.Response(200, json=load_fixture("stats.json"))
        if "/player/" in path:
            return httpx.Response(200, json=load_fixture("profile.json"))
        return httpx.Response(404)

    def client(self) -> ChessComClient:
        return ChessComClient(transport=httpx.MockTransport(self.handler), min_interval=0, sleep=lambda s: None)


@pytest.fixture
def db(tmp_path) -> Database:
    d = Database(tmp_path / "test.db")
    d.migrate()
    yield d
    d.close_all()


@pytest.fixture
def fake_chesscom() -> FakeChessCom:
    return FakeChessCom()


@pytest.fixture(scope="session")
def real_engine():
    if not stockfish_available():
        pytest.skip("Stockfish not installed")
    from plyback.analysis.engine import Engine

    e = Engine(threads=2, hash_mb=64)
    yield e
    e.close()
