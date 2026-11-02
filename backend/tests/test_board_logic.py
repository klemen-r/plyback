import chess
import pytest

from plyback.analysis import material as mat
from plyback.analysis.openings import from_chesscom_eco_url, identify, is_book_position
from plyback.analysis.pgn import PGNError, parse_pgn, parse_time_control, split_games, time_spent


def test_see_simple_and_defended():
    # White queen can take an undefended knight on d5.
    b = chess.Board("4k3/8/8/3n4/8/8/3Q4/4K3 w - - 0 1")
    assert mat.see(b, chess.D5) == 3
    # Knight defended by a pawn: QxN PxQ loses material, so best is not to capture.
    b = chess.Board("4k3/8/4p3/3n4/8/8/3Q4/4K3 w - - 0 1")
    assert mat.see(b, chess.D5) == 0


def test_see_respects_pins():
    # The white knight on d4 is pinned to the king by the rook on d8, so it can't capture on e6.
    b = chess.Board("3rk3/8/4b3/8/3N4/8/8/3K4 w - - 0 1")
    assert mat.see(b, chess.E6) == 0


def test_see_includes_promotion_and_restores_board():
    board = chess.Board("1r2k3/P7/8/8/8/8/8/4K3 w - - 0 1")
    original = board.fen()
    assert mat.see(board, chess.B8) == 13  # rook plus promotion from pawn to queen
    assert board.fen() == original
    assert not board.move_stack


@pytest.mark.parametrize("fen", [
    chess.STARTING_FEN,
    "1r2k3/P7/8/8/8/8/8/4K3 w - - 0 1",
    "4k3/8/8/8/8/8/Q~Q~6/4K3 w - - 0 1",
])
def test_material_counts_include_promoted_pieces(fen):
    board = chess.Board(fen)
    for color in chess.COLORS:
        expected = sum(mat.VALUES[p.piece_type] for p in board.piece_map().values() if p.color == color)
        assert mat.material(board, color) == expected
    expected = sum(
        mat.VALUES[p.piece_type] for p in board.piece_map().values() if p.piece_type not in (chess.PAWN, chess.KING)
    )
    assert mat.non_pawn_material(board) == expected


def test_hanging_pieces():
    b = chess.Board("4k3/8/8/3n4/8/8/3Q4/4K3 w - - 0 1")
    hanging = mat.hanging_pieces(b, chess.BLACK)
    assert hanging == [(chess.D5, 3)]


def test_sacrifice_detection():
    # Classic Bxf7+ — the bishop is taken by the king.
    b = chess.Board("r1bqk2r/pppp1ppp/2n2n2/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4")
    ok, desc = mat.is_sacrifice(b, chess.Move.from_uci("c4f7"))
    assert ok and "bishop" in desc
    # A plain trade is not a sacrifice.
    b = chess.Board("4k3/8/4p3/3n4/8/8/3Q4/4K3 w - - 0 1")
    ok, _ = mat.is_sacrifice(b, chess.Move.from_uci("d2d5"))
    assert ok  # queen for knight IS a sacrifice (net -6)
    b = chess.Board("4k3/8/4p3/3n4/8/2N5/8/4K3 w - - 0 1")
    ok, _ = mat.is_sacrifice(b, chess.Move.from_uci("c3d5"))
    assert not ok  # knight for knight


def test_already_hanging_piece_is_not_a_sacrifice():
    # Black's rook on h8 is already attacked by the knight on f7; Qxg2 doesn't offer it.
    b = chess.Board("r1b1kbnr/pppp1Npp/8/6q1/2BnP3/8/PPPP1PPP/RNBQK2R b KQkq - 0 5")
    ok, _ = mat.is_sacrifice(b, chess.Move.from_uci("g5g2"))
    assert not ok


def test_fork_targets():
    b = chess.Board("r3k3/8/8/8/8/8/8/1N2K3 w - - 0 1")
    mv = chess.Move.from_uci("b1c7")
    b2 = chess.Board("r3k3/8/8/8/8/8/8/4K3 w - - 0 1")
    b2.set_piece_at(chess.C7, chess.Piece(chess.KNIGHT, chess.WHITE))
    assert set(mat.fork_targets(b2, mv)) == {chess.A8, chess.E8}
    assert b  # silence unused


def test_phase():
    assert mat.phase(chess.Board(), 5) == "opening"
    assert mat.phase(chess.Board("4k3/pp6/8/8/8/8/PP6/4K2R w - - 0 40"), 79) == "endgame"
    mid = chess.Board("r1bq1rk1/ppp2ppp/2n2n2/3pp3/1bPP4/2N1PN2/PP3PPP/R1BQKB1R w KQ - 0 12")
    assert mat.phase(mid, 23) == "middlegame"


def test_material_words():
    assert mat.material_words(3) == "a piece"
    assert mat.material_words(-5) == "a rook"
    assert mat.material_words(9) == "the queen"


def test_opening_identification_and_transposition():
    def boards(moves):
        b = chess.Board()
        out = []
        for m in moves:
            b.push_san(m)
            out.append(b.copy())
        return out

    op, book = identify(boards(["e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5"]))
    assert op.family == "Italian Game"
    assert book == 6
    # Same position via a different move order.
    op2, _ = identify(boards(["Nf3", "Nc6", "e4", "e5", "Bc4", "Bc5"]))
    assert op2.family == "Italian Game"
    assert is_book_position(boards(["d4", "d5", "c4"])[-1])


def test_chesscom_eco_url_fallback():
    assert (
        from_chesscom_eco_url("https://www.chess.com/openings/Caro-Kann-Defense-Two-Knights-Attack...4.e5")
        == "Caro Kann Defense Two Knights Attack"
    )
    assert from_chesscom_eco_url(None) is None


def test_time_control_and_clocks():
    tc = parse_time_control("180+2")
    assert tc.base == 180 and tc.increment == 2 and not tc.daily
    assert parse_time_control("1/86400").daily
    pgn = '[TimeControl "180+2"]\n\n1. e4 {[%clk 0:03:01]} 1... e5 {[%clk 0:02:59]} 2. Nf3 {[%clk 0:02:55]} 2... Nc6 {[%clk 0:03:00]} *'
    g = parse_pgn(pgn)
    assert g.clocks == [181, 179, 175, 180]
    assert time_spent(g.clocks, tc) == [1.0, 3.0, 8.0, 1.0]


def test_parse_errors_and_split():
    with pytest.raises(PGNError):
        parse_pgn("1. e4 e5 2. Ke3 Ke6 3. Qxh8 *")
    text = '[Event "a"]\n\n1. e4 e5 *\n\n[Event "b"]\n\n1. d4 d5 *\n'
    assert len(split_games(text)) == 2


def test_chess960_parse():
    pgn = '[Variant "Chess960"]\n[SetUp "1"]\n[FEN "bqnbrkrn/pppppppp/8/8/8/8/PPPPPPPP/BQNBRKRN w GEge - 0 1"]\n\n1. e4 e5 *'
    g = parse_pgn(pgn)
    assert g.chess960 and g.plies == 2


def test_zwischenzug_is_not_a_sacrifice():
    # ...Nc5 leaves the f5 bishop en prise, but Bxf5 is met by Nxb3 winning the queen.
    b = chess.Board("r2qkb1r/ppp2ppp/2n5/3p1b2/4n3/1QPBPN1P/PP3PP1/RNB1K2R b KQkq - 3 8")
    ok, _ = mat.is_sacrifice(b, chess.Move.from_uci("e4c5"))
    assert not ok


def test_compiled_opening_index_matches_sources():
    from plyback.analysis.openings import _index, build_index

    named, book = build_index()
    compiled_named, compiled_book = _index()
    assert set(compiled_named) == set(named) and compiled_book == frozenset(book)
    k = next(iter(named))
    assert compiled_named[k].name == named[k].name
