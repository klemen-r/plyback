"""Game library, review data and PGN import."""

from __future__ import annotations

import chess
from fastapi import APIRouter, HTTPException, Query, Request
from pydantic import BaseModel, Field

from ..analysis.pgn import PGNError, parse_pgn, split_games
from ..analysis.winprob import win_pct_white
from ..config import ANALYSIS_VERSION
from ..db import loads
from ..services import store
from ..services.analysis import PRIORITY_MANUAL
from .deps import S

router = APIRouter()


def _job_map(s, ids: list[int]) -> dict[int, dict]:
    if not ids:
        return {}
    q = ",".join("?" for _ in ids)
    rows = s.db.query(
        f"""SELECT id, game_id, status, error FROM jobs WHERE game_id IN ({q})
                          AND status IN ('queued','running','failed') ORDER BY id""",
        ids,
    )
    out = {r["game_id"]: r for r in rows}
    for r in out.values():
        p = s.analysis.progress.get(r["id"])
        if p:
            r["progress"] = p["done"]
            r["total"] = p["total"]
    return out


def _game_summary(r: dict, job: dict | None) -> dict:
    counts = loads(r.get("counts_json"), None)
    user_color = r.get("user_color")
    outcome = None
    if user_color:
        res = r["white_result"] if user_color == "white" else r["black_result"]
        other = r["black_result"] if user_color == "white" else r["white_result"]
        outcome = "win" if res == "win" else "loss" if other == "win" else "draw"
    return {
        "id": r["id"],
        "url": r["url"],
        "source": r["source"],
        "end_time": r["end_time"],
        "time_class": r["time_class"],
        "time_control": r["time_control"],
        "rated": bool(r["rated"]) if r["rated"] is not None else None,
        "rules": r["rules"],
        "result": r["result"],
        "termination": r["termination"],
        "plies": r["plies"],
        "white": {
            "username": r["white_username"],
            "rating": r["white_rating"],
            "result": r["white_result"],
            "accuracy": r.get("accuracy_white"),
            "chesscom_accuracy": r["chesscom_accuracy_white"],
        },
        "black": {
            "username": r["black_username"],
            "rating": r["black_rating"],
            "result": r["black_result"],
            "accuracy": r.get("accuracy_black"),
            "chesscom_accuracy": r["chesscom_accuracy_black"],
        },
        "opening": {"eco": r["opening_eco"], "name": r["opening_name"], "family": r["opening_family"]},
        "user_color": user_color,
        "outcome": outcome,
        "analyzed": r.get("analyzed_at") is not None,
        "analysis_depth": r.get("depth"),
        "counts": counts,
        "job": {k: job[k] for k in ("status", "error", "progress", "total") if k in job} if job else None,
    }


@router.get("/games")
def list_games(
    request: Request,
    account_id: int | None = None,
    page: int = Query(1, ge=1),
    page_size: int = Query(50, ge=1, le=200),
    time_class: str | None = None,
    outcome: str | None = None,
    color: str | None = None,
    opening: str | None = None,
    analyzed: bool | None = None,
    q: str | None = None,
    since: int | None = None,
    until: int | None = None,
    rated: bool | None = None,
    classification: str | None = None,
    sort: str = "end_time_desc",
):
    s = S(request)
    where, params = [], []
    if account_id:
        join = "JOIN account_games ag ON ag.game_id = g.id AND ag.account_id = ?"
        params.append(account_id)
        user_color = "ag.color"
    else:
        join, user_color = "", "NULL"
    if time_class:
        where.append(f"g.time_class IN ({','.join('?' for _ in time_class.split(','))})")
        params += time_class.split(",")
    if color and account_id:
        where.append("ag.color = ?")
        params.append(color)
    if outcome and account_id:
        if outcome == "win":
            where.append("((ag.color='white' AND g.white_result='win') OR (ag.color='black' AND g.black_result='win'))")
        elif outcome == "loss":
            where.append("((ag.color='white' AND g.black_result='win') OR (ag.color='black' AND g.white_result='win'))")
        elif outcome == "draw":
            where.append("g.white_result != 'win' AND g.black_result != 'win'")
    if opening:
        where.append("g.opening_family = ?")
        params.append(opening)
    if analyzed is not None:
        where.append("ga.game_id IS NOT NULL" if analyzed else "ga.game_id IS NULL")
    if rated is not None:
        where.append("g.rated = ?")
        params.append(1 if rated else 0)
    if q:
        where.append("(g.white_username LIKE ? OR g.black_username LIKE ? OR g.opening_name LIKE ?)")
        params += [f"%{q}%"] * 3
    if since:
        where.append("g.end_time >= ?")
        params.append(since)
    if until:
        where.append("g.end_time < ?")
        params.append(until)
    if classification and account_id:
        where.append("""EXISTS (SELECT 1 FROM moves m WHERE m.game_id = g.id AND m.classification = ?
                        AND m.color = substr(ag.color, 1, 1))""")
        params.append(classification)
    wsql = ("WHERE " + " AND ".join(where)) if where else ""
    order = {
        "end_time_desc": "g.end_time DESC",
        "end_time_asc": "g.end_time ASC",
        "accuracy_desc": f"CASE {user_color} WHEN 'black' THEN ga.accuracy_black ELSE ga.accuracy_white END DESC",
        "accuracy_asc": f"CASE {user_color} WHEN 'black' THEN ga.accuracy_black ELSE ga.accuracy_white END ASC",
    }.get(sort, "g.end_time DESC")
    base = f"FROM games g {join} LEFT JOIN game_analysis ga ON ga.game_id = g.id {wsql}"
    total = s.db.scalar(f"SELECT COUNT(*) {base}", params)
    rows = s.db.query(
        f"""SELECT g.id, g.url, g.source, g.end_time, g.time_class, g.time_control, g.rated, g.rules, g.result,
                                 g.termination, g.plies, g.white_username, g.white_rating, g.white_result,
                                 g.black_username, g.black_rating, g.black_result, g.chesscom_accuracy_white,
                                 g.chesscom_accuracy_black, g.opening_eco, g.opening_name, g.opening_family,
                                 {user_color} AS user_color, ga.accuracy_white, ga.accuracy_black, ga.counts_json,
                                 ga.analyzed_at, ga.depth
                          {base} ORDER BY {order}, g.id DESC LIMIT ? OFFSET ?""",
        [*params, page_size, (max(1, page) - 1) * page_size],
    )
    jobs = _job_map(s, [r["id"] for r in rows])
    return {
        "total": total,
        "page": page,
        "page_size": page_size,
        "items": [_game_summary(r, jobs.get(r["id"])) for r in rows],
    }


@router.get("/games/openings")
def opening_names(request: Request, account_id: int | None = None):
    s = S(request)
    if account_id:
        rows = s.db.query(
            """SELECT g.opening_family AS family, COUNT(*) AS n FROM games g JOIN account_games ag
                             ON ag.game_id = g.id WHERE ag.account_id=? AND g.opening_family IS NOT NULL
                             GROUP BY 1 ORDER BY n DESC""",
            (account_id,),
        )
    else:
        rows = s.db.query("""SELECT opening_family AS family, COUNT(*) AS n FROM games WHERE opening_family IS NOT NULL
                             GROUP BY 1 ORDER BY n DESC""")
    return rows


@router.get("/games/{game_id}")
def get_game(request: Request, game_id: int, account_id: int | None = None):
    s = S(request)
    g = s.db.one("SELECT * FROM games WHERE id=?", (game_id,))
    if g is None:
        raise HTTPException(404, "Game not found")
    user_color = None
    if account_id:
        link = s.db.one("SELECT color FROM account_games WHERE account_id=? AND game_id=?", (account_id, game_id))
        user_color = link["color"] if link else None
    if user_color is None:
        link = s.db.one("SELECT color FROM account_games WHERE game_id=? ORDER BY account_id LIMIT 1", (game_id,))
        user_color = link["color"] if link else None
    ga = s.db.one("SELECT * FROM game_analysis WHERE game_id=?", (game_id,))
    job = _job_map(s, [game_id]).get(game_id)
    summary = _game_summary(
        {
            **g,
            "user_color": user_color,
            "accuracy_white": ga and ga["accuracy_white"],
            "accuracy_black": ga and ga["accuracy_black"],
            "counts_json": ga and ga["counts_json"],
            "analyzed_at": ga and ga["analyzed_at"],
            "depth": ga and ga["depth"],
        },
        job,
    )
    summary["pgn"] = g["pgn"]
    summary["initial_fen"] = g["initial_fen"]
    if ga:
        moves = []
        for m in s.db.query("SELECT * FROM moves WHERE game_id=? ORDER BY ply", (game_id,)):
            m.pop("game_id", None)
            m.pop("epd_before", None)
            m["tags"] = loads(m["tags"], {})
            m["lines"] = loads(m.pop("lines_json"), [])
            m["reply"] = loads(m.pop("reply_json"), None)
            m["is_book"] = bool(m["is_book"])
            moves.append(m)
        analysis = {
            "depth": ga["depth"],
            "multipv": ga["multipv"],
            "engine": ga["engine"],
            "version": ga["version"],
            "outdated": ga["version"] < ANALYSIS_VERSION,
            "analyzed_at": ga["analyzed_at"],
            "duration_ms": ga["duration_ms"],
            "accuracy": {"white": ga["accuracy_white"], "black": ga["accuracy_black"]},
            "acpl": {"white": ga["acpl_white"], "black": ga["acpl_black"]},
            "counts": loads(ga["counts_json"], {}),
            "opening": {
                "eco": ga["opening_eco"],
                "name": ga["opening_name"],
                "family": ga["opening_family"],
                "variation": ga["opening_variation"],
            },
            "book_plies": ga["book_plies"],
            "key_moments": loads(ga["key_moments_json"], []),
            "evals": loads(ga["evals_json"], []),
        }
        analysis["win_white"] = [round(win_pct_white(cp), 1) for cp in analysis["evals"]]
    else:
        analysis = None
        moves = []
        try:
            parsed = parse_pgn(g["pgn"])
            for i, mv in enumerate(parsed.moves):
                moves.append(
                    {
                        "ply": i + 1,
                        "color": "w" if parsed.boards[i].turn else "b",
                        "san": parsed.sans[i],
                        "uci": mv.uci(),
                        "fen_before": parsed.boards[i].fen(),
                        "clock": parsed.clocks[i],
                    }
                )
        except PGNError as exc:
            summary["pgn_error"] = str(exc)
    final_fen = None
    if moves:
        b = chess.Board(moves[-1]["fen_before"], chess960=g["rules"] == "chess960")
        b.push(chess.Move.from_uci(moves[-1]["uci"]))
        final_fen = b.fen()
    summary["final_fen"] = final_fen or g["initial_fen"]
    return {"game": summary, "analysis": analysis, "moves": moves}


class AnalyzeIn(BaseModel):
    depth: int | None = Field(None, ge=6, le=30)
    force: bool = False


@router.post("/games/{game_id}/analyze")
def analyze_game(request: Request, game_id: int, body: AnalyzeIn):
    s = S(request)
    if not s.db.one("SELECT id FROM games WHERE id=?", (game_id,)):
        raise HTTPException(404, "Game not found")
    added = s.analysis.enqueue([game_id], PRIORITY_MANUAL, depth=body.depth, force=body.force)
    return {"queued": bool(added)}


class ImportIn(BaseModel):
    pgn: str = Field(..., max_length=20_000_000)
    analyze: bool = True


@router.post("/games/import")
def import_pgn(request: Request, body: ImportIn):
    s = S(request)
    texts = split_games(body.pgn)
    if not texts:
        raise HTTPException(400, "No games found in the PGN")
    rows, errors = [], []
    for i, t in enumerate(texts):
        try:
            if parse_pgn(t).plies == 0:
                raise PGNError("Game has no moves")
            rows.append(store.game_row_from_pgn(t))
        except (PGNError, ValueError) as exc:
            errors.append({"index": i, "error": str(exc)})
    if not rows:
        raise HTTPException(400, errors[0]["error"] if errors else "No valid games found")
    accounts = s.db.query("SELECT id, username FROM accounts")
    with s.db.tx() as c:
        results = store.upsert_games(c, rows)
        ids = [gid for gid, _ in results]
        for acc in accounts:
            store.link_account_games(c, acc["id"], acc["username"], rows, ids)
    if body.analyze:
        s.analysis.enqueue(ids, PRIORITY_MANUAL)
    return {
        "imported": sum(1 for _, new in results if new),
        "existing": sum(1 for _, new in results if not new),
        "game_ids": ids,
        "errors": errors,
    }
