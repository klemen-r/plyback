"""Opt-in checks against the real Chess.com API:  pytest -m live"""

import pytest

from plyback.analysis.pgn import parse_pgn
from plyback.chesscom.client import ChessComClient, PlayerNotFound
from plyback.services.store import game_row_from_chesscom

pytestmark = pytest.mark.live
USER = "klemen_r"


def test_real_profile_archives_and_games():
    c = ChessComClient()
    try:
        assert c.profile(USER)["username"].lower() == USER
        archives = c.archives(USER).data["archives"]
        assert archives
        month = c.archive(archives[-1])
        games = month.data["games"]
        assert month.etag
        assert c.archive(archives[-1], etag=month.etag).status in (200, 304)
        for g in games[:10]:
            row = game_row_from_chesscom(g)
            assert row["uuid"] and row["result"] in ("1-0", "0-1", "1/2-1/2")
            if g.get("rules") == "chess":
                parsed = parse_pgn(g["pgn"])
                assert parsed.plies == row["plies"]
                assert any(cl is not None for cl in parsed.clocks) or g["time_class"] == "daily"
        with pytest.raises(PlayerNotFound):
            c.profile("this-user-should-not-exist-9x9x9")
    finally:
        c.close()
