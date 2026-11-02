"""Repository functions shared by services and API routes."""

from __future__ import annotations

import hashlib
import io
import logging
import re
import time
from datetime import datetime, timezone

import chess
import chess.pgn

from ..analysis.openings import from_chesscom_eco_url, identify
from ..config import ANALYSIS_VERSION, DEFAULT_SETTINGS, SETTING_LIMITS
from ..db import Database, dumps, loads

log = logging.getLogger(__name__)
HEADER_RE = re.compile(r'^\[(\w+)\s+"(.*)"\]\s*$', re.M)
DRAW_RESULTS = {"agreed", "repetition", "stalemate", "insufficient", "50move", "timevsinsufficient"}


# --- settings ---------------------------------------------------------------------------
def get_settings(db: Database) -> dict:
    out = dict(DEFAULT_SETTINGS)
    for row in db.query("SELECT key, value FROM settings"):
        if row["key"] in out:
            out[row["key"]] = loads(row["value"], out[row["key"]])
    return out


def validate_settings(patch: dict) -> dict:
    clean = {}
    for k, v in patch.items():
        if k not in DEFAULT_SETTINGS:
            raise ValueError(f"Unknown setting: {k}")
        default = DEFAULT_SETTINGS[k]
        if isinstance(default, bool):
            if not isinstance(v, bool):
                raise ValueError(f"{k} must be true/false")
        elif isinstance(default, (int, float)):
            if isinstance(v, bool) or not isinstance(v, (int, float)):
                raise ValueError(f"{k} must be a number")
            lo, hi = SETTING_LIMITS.get(k, (None, None))
            if lo is not None and not (lo <= v <= hi):
                raise ValueError(f"{k} must be between {lo} and {hi}")
            v = int(v) if isinstance(default, int) else float(v)
        elif isinstance(default, str):
            if not isinstance(v, str) or len(v) > 200:
                raise ValueError(f"{k} must be a short string")
        clean[k] = v
    return clean


def save_settings(db: Database, patch: dict) -> dict:
    clean = validate_settings(patch)
    with db.tx() as c:
        for k, v in clean.items():
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                (k, dumps(v)),
            )
    return get_settings(db)


# --- games ------------------------------------------------------------------------------
def pgn_headers(pgn: str) -> dict[str, str]:
    head = pgn.split("\n\n", 1)[0]
    return {m.group(1): m.group(2) for m in HEADER_RE.finditer(head)}


def _result(white_result: str | None, black_result: str | None, header: str | None) -> str:
    if white_result == "win":
        return "1-0"
    if black_result == "win":
        return "0-1"
    if white_result in DRAW_RESULTS or black_result in DRAW_RESULTS:
        return "1/2-1/2"
    return header or "*"


def _utc_ts(date: str | None, tm: str | None) -> int | None:
    if not date or not tm:
        return None
    try:
        return int(datetime.strptime(f"{date} {tm}", "%Y.%m.%d %H:%M:%S").replace(tzinfo=timezone.utc).timestamp())
    except ValueError:
        return None


def _count_plies(pgn: str, tcn: str | None) -> int:
    if tcn:
        return len(tcn) // 2
    body = pgn.split("\n\n", 1)[-1]
    body = re.sub(r"\{[^}]*\}", " ", body)
    body = re.sub(r"\d+\.(\.\.)?", " ", body)
    return len([t for t in body.split() if t not in ("1-0", "0-1", "1/2-1/2", "*")])


def game_row_from_chesscom(g: dict) -> dict | None:
    pgn = g.get("pgn")
    if not pgn:
        return None
    h = pgn_headers(pgn)
    white, black = g.get("white", {}), g.get("black", {})
    acc = g.get("accuracies") or {}
    return {
        "uuid": g.get("uuid") or hashlib.sha1((g.get("url") or pgn).encode(), usedforsecurity=False).hexdigest(),
        "url": g.get("url"),
        "source": "chesscom",
        "pgn": pgn,
        "rules": g.get("rules", "chess"),
        "time_control": g.get("time_control"),
        "time_class": g.get("time_class"),
        "rated": 1 if g.get("rated") else 0,
        "start_time": g.get("start_time") or _utc_ts(h.get("UTCDate"), h.get("UTCTime")),
        "end_time": g.get("end_time"),
        "white_username": white.get("username"),
        "white_rating": white.get("rating"),
        "white_result": white.get("result"),
        "black_username": black.get("username"),
        "black_rating": black.get("rating"),
        "black_result": black.get("result"),
        "result": _result(white.get("result"), black.get("result"), h.get("Result")),
        "termination": h.get("Termination"),
        "eco": h.get("ECO"),
        "eco_url": h.get("ECOUrl") or g.get("eco"),
        "initial_fen": g.get("initial_setup") or h.get("FEN") or chess.STARTING_FEN,
        "plies": _count_plies(pgn, g.get("tcn")),
        "chesscom_accuracy_white": acc.get("white"),
        "chesscom_accuracy_black": acc.get("black"),
        "imported_at": int(time.time()),
    }


def game_row_from_pgn(pgn: str) -> dict:
    """Row for a PGN imported manually (not from the Chess.com API)."""
    h = pgn_headers(pgn)
    res = h.get("Result", "*")
    wr = {"1-0": "win", "0-1": "lose", "1/2-1/2": "agreed"}.get(res)
    br = {"1-0": "lose", "0-1": "win", "1/2-1/2": "agreed"}.get(res)
    tc = h.get("TimeControl")
    end = _utc_ts(h.get("EndDate") or h.get("UTCDate") or h.get("Date"), h.get("EndTime", "").split(" ")[0] or "00:00:00")
    time_class = None
    if tc and "/" in tc:
        time_class = "daily"
    elif tc:
        try:
            parts = tc.split("+")
            base = float(parts[0]) + 40 * float(parts[1] if len(parts) > 1 else 0)
            time_class = "bullet" if base < 180 else "blitz" if base < 600 else "rapid"
        except ValueError:
            time_class = None
    link = h.get("Link") or None
    return {
        "uuid": "pgn-" + hashlib.sha1(pgn.strip().encode(), usedforsecurity=False).hexdigest()[:24],
        "url": link,
        "source": "pgn",
        "pgn": pgn.strip() + "\n",
        "rules": "chess960" if h.get("Variant", "").lower() == "chess960" else "chess",
        "time_control": tc,
        "time_class": time_class,
        "rated": None,
        "start_time": _utc_ts(h.get("UTCDate"), h.get("UTCTime")),
        "end_time": end,
        "white_username": h.get("White"),
        "white_rating": int(h["WhiteElo"]) if h.get("WhiteElo", "").isdigit() else None,
        "white_result": wr,
        "black_username": h.get("Black"),
        "black_rating": int(h["BlackElo"]) if h.get("BlackElo", "").isdigit() else None,
        "black_result": br,
        "result": res,
        "termination": h.get("Termination"),
        "eco": h.get("ECO"),
        "eco_url": h.get("ECOUrl"),
        "initial_fen": h.get("FEN") or chess.STARTING_FEN,
        "plies": _count_plies(pgn, None),
        "chesscom_accuracy_white": None,
        "chesscom_accuracy_black": None,
        "imported_at": int(time.time()),
    }


GAME_COLS = [
    "uuid",
    "url",
    "source",
    "pgn",
    "rules",
    "time_control",
    "time_class",
    "rated",
    "start_time",
    "end_time",
    "white_username",
    "white_rating",
    "white_result",
    "black_username",
    "black_rating",
    "black_result",
    "result",
    "termination",
    "eco",
    "eco_url",
    "initial_fen",
    "plies",
    "first_moves",
    "opening_eco",
    "opening_name",
    "opening_family",
    "opening_variation",
    "chesscom_accuracy_white",
    "chesscom_accuracy_black",
    "imported_at",
]


def enrich_opening(row: dict) -> dict:
    """Identify the opening (engine-free) and the first moves; tolerant of bad PGNs."""
    row.setdefault("first_moves", None)
    for k in ("opening_eco", "opening_name", "opening_family", "opening_variation"):
        row.setdefault(k, None)
    if row.get("rules") not in ("chess", None):
        return row
    try:
        game = chess.pgn.read_game(io.StringIO(row["pgn"]))
        if game is None:
            return row
        board = game.board()
        boards, sans = [], []
        for i, mv in enumerate(game.mainline_moves()):
            if i >= 40:
                break
            if i < 2:
                sans.append(board.san(mv))
            board.push(mv)
            boards.append(board.copy(stack=False))
        row["first_moves"] = " ".join(sans) or None
        op, _ = identify(boards)
        if op is not None:
            row.update(opening_eco=op.eco, opening_name=op.name, opening_family=op.family, opening_variation=op.variation)
        else:
            fallback = from_chesscom_eco_url(row.get("eco_url"))
            if fallback:
                row.update(opening_eco=row.get("eco"), opening_name=fallback, opening_family=fallback, opening_variation="")
    except Exception:  # never let a malformed PGN break an import
        log.debug("Opening detection failed for %s", row.get("uuid"), exc_info=True)
    return row


def upsert_games(c, rows: list[dict]) -> list[tuple[int, bool]]:
    """Insert games (inside an open transaction). Returns [(game_id, is_new)]."""
    out = []
    placeholders = ",".join("?" for _ in GAME_COLS)
    for r in rows:
        existing = c.execute("SELECT id, chesscom_accuracy_white FROM games WHERE uuid=?", (r["uuid"],)).fetchone()
        if existing:
            if r.get("chesscom_accuracy_white") is not None and existing["chesscom_accuracy_white"] is None:
                c.execute(
                    "UPDATE games SET chesscom_accuracy_white=?, chesscom_accuracy_black=? WHERE id=?",
                    (r["chesscom_accuracy_white"], r["chesscom_accuracy_black"], existing["id"]),
                )
            out.append((existing["id"], False))
            continue
        enrich_opening(r)
        cur = c.execute(f"INSERT INTO games ({','.join(GAME_COLS)}) VALUES ({placeholders})", [r[k] for k in GAME_COLS])
        out.append((cur.lastrowid, True))
    return out


def link_account_games(c, account_id: int, username: str, rows: list[dict], ids: list[int]) -> None:
    u = username.lower()
    for r, gid in zip(rows, ids, strict=True):
        if (r.get("white_username") or "").lower() == u:
            color = "white"
        elif (r.get("black_username") or "").lower() == u:
            color = "black"
        else:
            continue
        c.execute(
            "INSERT OR IGNORE INTO account_games (account_id, game_id, color) VALUES (?,?,?)", (account_id, gid, color)
        )


# --- analysis ---------------------------------------------------------------------------
def save_analysis(db: Database, game_id: int, result: dict, depth: int, multipv: int, engine: str) -> None:
    op = result.get("opening") or {}
    with db.tx() as c:
        c.execute("DELETE FROM moves WHERE game_id=?", (game_id,))
        c.executemany(
            """INSERT INTO moves (game_id, ply, color, san, uci, fen_before, epd_before, eval_before, eval_after,
               win_before, win_after, ep_loss, cp_loss, accuracy, best_uci, best_san, classification, phase, clock,
               time_spent, is_book, tags, explanation, lines_json, reply_json, endgame_type)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            [
                (
                    game_id,
                    m["ply"],
                    m["color"],
                    m["san"],
                    m["uci"],
                    m["fen_before"],
                    " ".join(m["fen_before"].split()[:4]),
                    m["eval_before"],
                    m["eval_after"],
                    m["win_before"],
                    m["win_after"],
                    m["ep_loss"],
                    m["cp_loss"],
                    m["accuracy"],
                    m["best_uci"],
                    m["best_san"],
                    m["classification"],
                    m["phase"],
                    m["clock"],
                    m["time_spent"],
                    1 if m["is_book"] else 0,
                    dumps(m["tags"]),
                    m["explanation"],
                    dumps(m["lines"]),
                    dumps(m["reply"]),
                    m.get("endgame_type"),
                )
                for m in result["moves"]
            ],
        )
        c.execute(
            """INSERT INTO game_analysis (game_id, depth, multipv, engine, version, analyzed_at, duration_ms,
               accuracy_white, accuracy_black, acpl_white, acpl_black, opening_eco, opening_name, opening_family,
               opening_variation, book_plies, counts_json, key_moments_json, evals_json)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
               ON CONFLICT(game_id) DO UPDATE SET depth=excluded.depth, multipv=excluded.multipv, engine=excluded.engine,
               revision=game_analysis.revision+1,
               version=excluded.version, analyzed_at=excluded.analyzed_at, duration_ms=excluded.duration_ms,
               accuracy_white=excluded.accuracy_white, accuracy_black=excluded.accuracy_black,
               acpl_white=excluded.acpl_white, acpl_black=excluded.acpl_black, opening_eco=excluded.opening_eco,
               opening_name=excluded.opening_name, opening_family=excluded.opening_family,
               opening_variation=excluded.opening_variation, book_plies=excluded.book_plies,
               counts_json=excluded.counts_json, key_moments_json=excluded.key_moments_json,
               evals_json=excluded.evals_json""",
            (
                game_id,
                depth,
                multipv,
                engine,
                ANALYSIS_VERSION,
                int(time.time()),
                result.get("duration_ms"),
                result["accuracy_white"],
                result["accuracy_black"],
                result["acpl_white"],
                result["acpl_black"],
                op.get("eco"),
                op.get("name"),
                op.get("family"),
                op.get("variation"),
                result.get("book_plies"),
                dumps(result["counts"]),
                dumps(result["key_moments"]),
                dumps(result["evals"]),
            ),
        )


def account_by_id(db: Database, account_id: int) -> dict | None:
    return db.one("SELECT * FROM accounts WHERE id=?", (account_id,))


def account_public(db: Database, acc: dict) -> dict:
    stats = loads(acc.get("stats_json"), {}) or {}
    ratings = {}
    for key, val in stats.items():
        if key.startswith("chess_") and isinstance(val, dict):
            ratings[key.removeprefix("chess_")] = {
                "rating": (val.get("last") or {}).get("rating"),
                "best": (val.get("best") or {}).get("rating"),
                "record": val.get("record"),
            }
    counts = (
        db.one(
            """SELECT COUNT(*) AS games, SUM(CASE WHEN ga.game_id IS NOT NULL THEN 1 ELSE 0 END) AS analyzed
           FROM account_games ag LEFT JOIN game_analysis ga ON ga.game_id = ag.game_id WHERE ag.account_id=?""",
            (acc["id"],),
        )
        or {}
    )
    queued = db.scalar(
        """SELECT COUNT(*) FROM jobs j JOIN account_games ag ON ag.game_id=j.game_id
           WHERE ag.account_id=? AND j.status IN ('queued','running')""",
        (acc["id"],),
    )
    profile = loads(acc.get("profile_json"), {}) or {}
    return {
        "id": acc["id"],
        "username": acc["username"],
        "display_name": acc.get("display_name") or acc["username"],
        "avatar": acc.get("avatar"),
        "title": profile.get("title"),
        "country": (profile.get("country") or "").rsplit("/", 1)[-1] or None,
        "profile_url": profile.get("url") or f"https://www.chess.com/member/{acc['username']}",
        "joined": profile.get("joined"),
        "ratings": ratings,
        "added_at": acc["added_at"],
        "last_sync_at": acc.get("last_sync_at"),
        "sync_status": acc.get("sync_status"),
        "sync_error": acc.get("sync_error"),
        "sync_enabled": bool(acc.get("sync_enabled")),
        "auto_analyze": bool(acc.get("auto_analyze")),
        "analyze_history": bool(acc.get("analyze_history")),
        "import_since": acc.get("import_since"),
        "games": counts.get("games") or 0,
        "analyzed": counts.get("analyzed") or 0,
        "queued": queued or 0,
    }
