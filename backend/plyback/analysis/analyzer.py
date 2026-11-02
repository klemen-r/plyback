"""Full-game analysis: engine evaluation of every position, move classification,
tactical tagging, explanations, accuracy and centipawn loss.

The analyser is engine-agnostic: it receives an ``evaluate(board, depth, multipv)``
callable so it can be tested with deterministic fakes and so callers can add caching.

Classification (all thresholds use *expected points* = win% / 100, mover's view;
``loss`` = win%(best move) − win%(played move)):

==============  ===============================================================
book            position after the move is in the opening database (continuous from move 1)
forced          the only legal move
brilliant       the engine's best move AND sacrifices ≥ 2 points of material AND the mover is
                at least equal afterwards AND the position was not already decided (second-best
                < 80%), and the opponent can't take the material without immediately losing
                at least as much back — unless the sacrifice forces mate where the alternatives don't
great           the engine's best move AND every alternative is ≥ 15% worse (an "only move")
                AND the mover is not lost afterwards; excludes plain recaptures, grabbing
                hanging material, checkmates, queen promotions and trivial check evasions
best            the engine's top choice
excellent       loss < 2%
good            loss < 5%
inaccuracy      loss < 10%
mistake         loss < 20%
blunder         loss ≥ 20%
miss            a mistake/blunder made right after the opponent erred (≥ 10% swing in your
                favour) that throws the gift back without making your position worse than
                before the opponent's error
==============  ===============================================================
"""

from __future__ import annotations

import logging
import time
from collections.abc import Callable
from dataclasses import dataclass

import chess

from . import material as mat
from .engine import Line
from .explain import explain
from .openings import identify
from .pgn import ParsedGame, parse_time_control, time_spent
from .winprob import cp_loss, format_eval, game_accuracy, mate_in, move_accuracy, win_pct

log = logging.getLogger(__name__)

Evaluate = Callable[[chess.Board, int, int], list[Line]]

THRESHOLDS = [(0.02, "excellent"), (0.05, "good"), (0.10, "inaccuracy"), (0.20, "mistake")]
GREAT_GAP = 15.0
MISS_SWING = 10.0
THREAT_DROP = 15.0
TACTICAL_TAGS = {
    "hanging_piece",
    "allows_fork",
    "missed_fork",
    "missed_material",
    "loses_material",
    "allowed_mate",
    "missed_mate",
    "threat_ignored",
}
ERROR_CLASSES = {"inaccuracy", "mistake", "blunder", "miss"}


@dataclass
class AnalysisOptions:
    depth: int = 16
    multipv: int = 2
    threat_depth: int = 10


def pv_to_san(board: chess.Board, uci_moves: list[str], limit: int = 10) -> list[str]:
    b = board.copy(stack=False)
    out = []
    for u in uci_moves[:limit]:
        try:
            mv = chess.Move.from_uci(u)
            if mv not in b.legal_moves:
                break
            out.append(b.san(mv))
            b.push(mv)
        except ValueError:
            break
    return out


def _play_line(board: chess.Board, uci_moves: list[str], max_plies: int = 8) -> tuple[chess.Board, int]:
    """Play a PV, stopping at a quiet point (not mid-exchange). Returns (board, plies played)."""
    b = board.copy(stack=False)
    played = 0
    for i, u in enumerate(uci_moves):
        mv = chess.Move.from_uci(u)
        if mv not in b.legal_moves:
            break
        if i >= max_plies:
            # Allow finishing an exchange that is in progress.
            if not (b.is_capture(mv) and i < max_plies + 2):
                break
        b.push(mv)
        played += 1
    return b, played


def _is_recapture(prev_board: chess.Board | None, prev_move: chess.Move | None, move: chess.Move) -> bool:
    return bool(
        prev_board is not None
        and prev_move is not None
        and prev_board.is_capture(prev_move)
        and move.to_square == prev_move.to_square
    )


def _mate_for(cp_white: int, white: bool) -> int | None:
    """Moves to mate for the given side (positive = that side mates), else None."""
    m = mate_in(cp_white)
    if m is None:
        return None
    return m if white else -m


def analyze_parsed(
    game: ParsedGame,
    evaluate: Evaluate,
    opts: AnalysisOptions | None = None,
    progress: Callable[[int, int], None] | None = None,
    time_control: str | None = None,
) -> dict:
    opts = opts or AnalysisOptions()
    t0 = time.time()
    boards = game.boards
    n = game.plies
    evals: list[list[Line]] = []
    for i, b in enumerate(boards):
        evals.append(evaluate(b, opts.depth, opts.multipv))
        if progress:
            progress(i + 1, n + 1)

    # A position's PVs serve both its alternatives and the preceding move's reply.
    san_evals = [
        [pv_to_san(board, line.pv, 12) for line in lines]
        for board, lines in zip(boards, evals, strict=True)
    ]
    fens = [board.fen() for board in boards]
    opening, book_plies = identify(boards[1:])
    tc = parse_time_control(time_control or game.headers.get("TimeControl"))
    spent = time_spent(game.clocks, tc)
    moves_out: list[dict] = []
    counts = {"white": {}, "black": {}}

    for i, move in enumerate(game.moves):
        before, after = boards[i], boards[i + 1]
        white = before.turn == chess.WHITE
        lines_b, lines_a = evals[i], evals[i + 1]
        best = lines_b[0]
        best_cp, after_cp = best.cp, lines_a[0].cp
        best_uci = best.pv[0] if best.pv else None
        played_is_best = best_uci == move.uci()
        win_b = win_pct(best_cp, white)
        win_a = win_pct(after_cp, white)
        loss = 0.0 if played_is_best else max(0.0, win_b - win_a)
        second = lines_b[1] if len(lines_b) > 1 else None
        if second is not None and second.pv and second.pv[0] == move.uci():
            second = lines_b[0]  # played the 2nd line; "alternative" is then the engine's first choice
        win_second = win_pct(second.cp, white) if second is not None else None
        n_legal = before.legal_moves.count()
        prev_board = boards[i - 1] if i > 0 else None
        prev_move = game.moves[i - 1] if i > 0 else None
        win_prev = win_pct(evals[i - 1][0].cp, white) if i > 0 else None
        ply = i + 1  # index into the game (1 = first move played), used by the UI
        abs_ply = 2 * (boards[0].fullmove_number - 1) + (1 if boards[0].turn == chess.BLACK else 0) + ply
        in_book = ply <= book_plies
        phase = mat.phase(before, abs_ply, in_book)
        tags: dict = {}

        # --- classification -------------------------------------------------------------
        if n_legal == 1:
            cls = "forced"
        elif in_book:
            cls = "book"
        else:
            cls = None
            if played_is_best:
                # Not already decided: the alternatives must not also be crushing, unless the
                # sacrifice forces mate where the alternatives don't.
                best_mates = (_mate_for(best_cp, white) or 0) > 0
                second_mates = second is not None and (_mate_for(second.cp, white) or 0) > 0
                undecided = (win_second is None and win_b < 80) or (win_second is not None and win_second < 80)
                if (undecided or (best_mates and not second_mates)) and win_a >= 50:
                    sac, sac_desc = mat.is_sacrifice(before, move)
                    if sac:
                        cls = "brilliant"
                        tags["sacrifice"] = {"piece": sac_desc}
            if cls is None and played_is_best:
                cls = "best"
                if win_second is not None and win_b - win_second >= GREAT_GAP and win_a >= 35 and win_second < 90:
                    trivial = (
                        _is_recapture(prev_board, prev_move, move)
                        or after.is_checkmate()
                        or (before.is_check() and n_legal <= 2)
                        or move.promotion == chess.QUEEN
                        or (before.is_capture(move) and mat.see(before.copy(stack=False), move.to_square) >= 2)
                    )
                    if not trivial:
                        cls = "great"
            if cls is None:
                cls = "blunder"
                for limit, name in THRESHOLDS:
                    if loss / 100 < limit:
                        cls = name
                        break
                if cls in ("mistake", "blunder") and win_prev is not None:
                    if win_b - win_prev >= MISS_SWING and win_a >= win_prev - 5:
                        cls = "miss"
                        tags["missed_punish"] = {}

        # --- tactical facts for errors and missed wins -------------------------------------
        best_line_san = san_evals[i][0][:10]
        reply = lines_a[0]
        reply_san = san_evals[i + 1][0][:10]
        mover_mate_best = _mate_for(best_cp, white)
        if mover_mate_best is not None and 0 < mover_mate_best <= 3 and not played_is_best and not after.is_checkmate():
            mover_mate_after = _mate_for(after_cp, white)
            if mover_mate_after is None or mover_mate_after <= 0:
                tags["missed_mate"] = {"n": mover_mate_best}
        if cls in ERROR_CLASSES or (cls in ("good", "excellent") and "missed_mate" in tags):
            _tactical_tags(before, after, move, white, best, reply, best_cp, after_cp, tags)
            if cls in ("mistake", "blunder", "miss") and not before.is_check() and "allowed_mate" not in tags:
                _threat_test(before, move, reply, white, win_b, evaluate, opts, tags)
            if before.is_check():
                tags["in_check"] = {}
            if cls in ("mistake", "blunder") and not (tags.keys() & {"threat_ignored", "missed_punish"}):
                tags["self_inflicted"] = {}
            tags["tactical" if tags.keys() & TACTICAL_TAGS else "positional"] = {}

        alt_lines = []
        for ln, line_san in zip(lines_b, san_evals[i], strict=True):
            alt_lines.append(
                {
                    "uci": ln.pv[0] if ln.pv else None,
                    "cp": ln.cp,
                    "eval": format_eval(ln.cp),
                    "san": line_san,
                    "depth": ln.depth,
                }
            )
        second_san = None
        if second is not None and second.pv:
            ss = san_evals[i][lines_b.index(second)]
            second_san = ss[0] if ss else None
        ctx = {
            "cls": cls,
            "san": game.sans[i],
            "best_san": best_line_san[0] if best_line_san else None,
            "best_line": best_line_san,
            "reply_line": reply_san,
            "eval_best": best_cp,
            "eval_after": after_cp,
            "tags": tags,
            "opening": opening.name if opening else None,
            "second_san": second_san,
            "eval_second": second.cp if second is not None else None,
            "white": white,
            "win_after": win_a,
            "win_before": win_b,
        }
        clock = game.clocks[i]
        if clock is not None and tc.base and not tc.daily and clock <= max(10.0, tc.base * 0.1):
            tags["time_pressure"] = {}
        color = "white" if white else "black"
        counts[color][cls] = counts[color].get(cls, 0) + 1
        moves_out.append(
            {
                "ply": ply,
                "move_number": before.fullmove_number,
                "color": "w" if white else "b",
                "san": game.sans[i],
                "uci": move.uci(),
                "fen_before": fens[i],
                "fen_after": fens[i + 1],
                "eval_before": best_cp,
                "eval_after": after_cp,
                "win_before": round(win_b, 2),
                "win_after": round(win_a, 2),
                "ep_loss": round(loss / 100, 4),
                "cp_loss": 0 if played_is_best else cp_loss(best_cp, after_cp, white),
                "accuracy": round(100.0 if played_is_best else move_accuracy(win_b, win_a), 1),
                "best_uci": best_uci,
                "best_san": best_line_san[0] if best_line_san else None,
                "lines": alt_lines,
                "reply": {"uci": reply.pv[0] if reply.pv else None, "san": reply_san, "cp": reply.cp},
                "classification": cls,
                "phase": phase,
                "endgame_type": mat.endgame_type(fens[i]) if phase == "endgame" else None,
                "clock": clock,
                "time_spent": spent[i] if i < len(spent) else None,
                "is_book": in_book,
                "tags": tags,
                "explanation": explain(ctx),
            }
        )

    pos_evals = [e[0].cp for e in evals]
    acc_w, acc_b = game_accuracy(pos_evals, boards[0].turn == chess.WHITE)

    def acpl(color: str) -> float | None:
        xs = [m["cp_loss"] for m in moves_out if m["color"] == color and not m["is_book"]]
        return round(sum(xs) / len(xs), 1) if xs else None

    return {
        "evals": pos_evals,
        "moves": moves_out,
        "accuracy_white": acc_w,
        "accuracy_black": acc_b,
        "acpl_white": acpl("w"),
        "acpl_black": acpl("b"),
        "counts": counts,
        "opening": opening.as_dict() if opening else None,
        "book_plies": book_plies,
        "key_moments": _key_moments(moves_out),
        "duration_ms": int((time.time() - t0) * 1000),
    }


def _tactical_tags(
    before: chess.Board,
    after: chess.Board,
    move: chess.Move,
    white: bool,
    best: Line,
    reply: Line,
    best_cp: int,
    after_cp: int,
    tags: dict,
) -> None:
    mover = before.turn
    start_bal = mat.balance(before, mover)
    # Opponent's best reply line after the played move.
    reply_end, reply_plies = _play_line(after, reply.pv, 6)
    played_bal = mat.balance(reply_end, mover)
    best_end, best_plies = _play_line(before, best.pv, 7)
    best_bal = mat.balance(best_end, mover)

    opp_mate = _mate_for(after_cp, not white)
    opp_mate_before = _mate_for(best_cp, not white)
    if opp_mate is not None and opp_mate > 0 and (opp_mate_before is None or opp_mate_before <= 0):
        tags["allowed_mate"] = {"n": opp_mate}
        mate_board, _ = _play_line(after, reply.pv, 2 * opp_mate + 1)
        if mate_board.is_checkmate():
            ksq = mate_board.king(mover)
            back = chess.square_rank(ksq) == (0 if mover == chess.WHITE else 7) if ksq is not None else False
            checkers = [mate_board.piece_at(s) for s in mate_board.checkers()]
            if back and any(
                c and c.piece_type in (chess.ROOK, chess.QUEEN) and chess.square_rank(s) == chess.square_rank(ksq)
                for c, s in zip(checkers, mate_board.checkers(), strict=True)
            ):
                tags["back_rank"] = {}

    lost = start_bal - played_bal
    if reply.pv and lost >= 2:
        r1 = chess.Move.from_uci(reply.pv[0])
        victim = after.piece_at(r1.to_square) if r1 in after.legal_moves else None
        if victim is not None and victim.color == mover and after.is_capture(r1) and mat.VALUES[victim.piece_type] >= 2:
            tags["hanging_piece"] = {
                "piece": mat.PIECE_NAMES[victim.piece_type],
                "square": chess.square_name(r1.to_square),
                "capture": after.san(r1),
                "value": mat.VALUES[victim.piece_type],
            }
        else:
            if r1 in after.legal_moves:
                b2 = after.copy(stack=False)
                b2.push(r1)
                targets = mat.fork_targets(b2, r1)
                if targets:
                    tags["allows_fork"] = {
                        "move": after.san(r1),
                        "targets": [mat.PIECE_NAMES[b2.piece_at(s).piece_type] for s in targets],
                    }
        tags["loses_material"] = {"points": lost, "words": mat.material_words(lost), "plies": reply_plies}

    gain_missed = best_bal - played_bal
    if gain_missed >= 2 and best_bal - start_bal >= 2:
        tags["missed_material"] = {"points": best_bal - start_bal, "words": mat.material_words(best_bal - start_bal)}
        if best.pv:
            b1 = chess.Move.from_uci(best.pv[0])
            if b1 in before.legal_moves:
                b2 = before.copy(stack=False)
                b2.push(b1)
                targets = mat.fork_targets(b2, b1)
                if targets:
                    tags["missed_fork"] = {
                        "move": before.san(b1),
                        "targets": [mat.PIECE_NAMES[b2.piece_at(s).piece_type] for s in targets],
                    }


def _threat_test(
    before: chess.Board,
    move: chess.Move,
    reply: Line,
    white: bool,
    win_b: float,
    evaluate: Evaluate,
    opts: AnalysisOptions,
    tags: dict,
) -> None:
    """Null-move test: if passing would let the opponent gain ≥ 15% there was a threat;
    flag it as ignored when the opponent's refutation executes the same idea."""
    nb = before.copy(stack=False)
    nb.turn = not nb.turn
    nb.ep_square = None
    if nb.is_check() or not any(True for _ in nb.legal_moves):
        return
    try:
        lines = evaluate(nb, opts.threat_depth, 1)
    except Exception:  # threat detection is best-effort
        return
    if not lines or not lines[0].pv:
        return
    win_null = win_pct(lines[0].cp, white)
    if win_b - win_null < THREAT_DROP:
        return
    threat = chess.Move.from_uci(lines[0].pv[0])
    r1 = chess.Move.from_uci(reply.pv[0]) if reply.pv else None
    same = r1 is not None and (
        r1 == threat
        or (r1.to_square == threat.to_square and r1.from_square == threat.from_square)
        or (r1.to_square == threat.to_square and nb.is_capture(threat))
    )
    if same:
        tags["threat_ignored"] = {"move": nb.san(threat)}


def _key_moments(moves: list[dict], limit: int = 6) -> list[int]:
    """Plies of the most consequential moves (biggest errors plus brilliant/great finds)."""
    scored = []
    for m in moves:
        if m["classification"] in ("brilliant", "great"):
            scored.append((0.5 + (1 if m["classification"] == "brilliant" else 0), m["ply"]))
        elif m["classification"] in ERROR_CLASSES:
            scored.append((m["ep_loss"], m["ply"]))
    scored.sort(reverse=True)
    return sorted(p for _, p in scored[:limit])
