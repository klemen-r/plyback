"""Chess.com account management."""

from __future__ import annotations

import re
import sqlite3
import time

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from ..chesscom.client import ChessComError, PlayerNotFound
from ..db import dumps
from ..insights.compute import Filters
from ..services import store
from ..services.analysis import PRIORITY_REPORT
from ..services.reports import scope_game_ids
from .deps import S, account_or_404

router = APIRouter()
USERNAME_RE = re.compile(r"^[A-Za-z0-9_-]{2,40}$")


class AccountIn(BaseModel):
    username: str
    import_since: str | None = Field(None, description="YYYY/MM; omit for full history")
    analyze_history: bool = True
    auto_analyze: bool = True


class AccountPatch(BaseModel):
    sync_enabled: bool | None = None
    auto_analyze: bool | None = None
    analyze_history: bool | None = None
    import_since: str | None = None


@router.get("/accounts")
def list_accounts(request: Request):
    s = S(request)
    return [store.account_public(s.db, a) for a in s.db.query("SELECT * FROM accounts ORDER BY added_at")]


@router.post("/accounts", status_code=201)
def add_account(request: Request, body: AccountIn):
    s = S(request)
    username = body.username.strip().lstrip("@")
    if not USERNAME_RE.match(username):
        raise HTTPException(400, "That doesn't look like a valid Chess.com username")
    if body.import_since and not re.fullmatch(r"(?!0000)\d{4}/(?:0[1-9]|1[0-2])", body.import_since):
        raise HTTPException(400, "import_since must look like YYYY/MM")
    if s.db.one("SELECT id FROM accounts WHERE username=?", (username,)):
        raise HTTPException(409, f"{username} is already connected")
    try:
        profile = s.sync.client.profile(username)
    except PlayerNotFound:
        raise HTTPException(404, f"No Chess.com player named '{username}'") from None
    except ChessComError as exc:
        raise HTTPException(502, f"Chess.com is not reachable right now: {exc}") from exc
    try:
        with s.db.tx() as c:
            cur = c.execute(
                """INSERT INTO accounts (username, display_name, avatar, profile_json, added_at, auto_analyze,
                           analyze_history, import_since) VALUES (?,?,?,?,?,?,?,?)""",
                (
                    profile.get("username") or username,
                    profile.get("username") or username,
                    profile.get("avatar"),
                    dumps(profile),
                    int(time.time()),
                    int(body.auto_analyze),
                    int(body.analyze_history),
                    body.import_since,
                ),
            )
            account_id = cur.lastrowid
    except sqlite3.IntegrityError as exc:
        raise HTTPException(409, f"{username} is already connected") from exc
    s.db.log_event("info", "accounts", f"Connected {username}", account_id)
    s.sync.request(account_id)
    return store.account_public(s.db, store.account_by_id(s.db, account_id))


@router.get("/accounts/{account_id}")
def get_account(request: Request, account_id: int):
    s = S(request)
    return store.account_public(s.db, account_or_404(s, account_id))


@router.patch("/accounts/{account_id}")
def patch_account(request: Request, account_id: int, body: AccountPatch):
    s = S(request)
    account_or_404(s, account_id)
    fields = body.model_dump(exclude_unset=True)
    if any(v is None for k, v in fields.items() if k != "import_since"):
        raise HTTPException(422, "Account switches must be true or false")
    if (
        "import_since" in fields
        and fields["import_since"]
        and not re.fullmatch(r"(?!0000)\d{4}/(?:0[1-9]|1[0-2])", fields["import_since"])
    ):
        raise HTTPException(400, "import_since must look like YYYY/MM")
    if fields:
        sets = ", ".join(f"{k}=?" for k in fields)
        vals = [int(v) if isinstance(v, bool) else v for v in fields.values()]
        s.db.execute(f"UPDATE accounts SET {sets} WHERE id=?", (*vals, account_id))
        if "import_since" in fields:  # re-scan archives that may now be in range
            s.db.execute("UPDATE archives SET complete=0 WHERE account_id=?", (account_id,))
            s.db.execute("UPDATE accounts SET history_complete=0 WHERE id=?", (account_id,))
            s.sync.request(account_id)
        if fields.get("analyze_history"):
            s.sync.backfill(account_id)
    return store.account_public(s.db, store.account_by_id(s.db, account_id))


@router.delete("/accounts/{account_id}")
def delete_account(request: Request, account_id: int):
    s = S(request)
    account_or_404(s, account_id)
    s.analysis.cancel_for_account(account_id)
    with s.db.tx() as c:
        orphan_ids = [
            r[0]
            for r in c.execute(
                """SELECT game_id FROM account_games WHERE account_id=? AND game_id NOT IN
               (SELECT game_id FROM account_games WHERE account_id != ?)""",
                (account_id, account_id),
            ).fetchall()
        ]
        c.execute("DELETE FROM accounts WHERE id=?", (account_id,))
        for i in range(0, len(orphan_ids), 500):
            chunk = orphan_ids[i : i + 500]
            c.execute(f"DELETE FROM games WHERE id IN ({','.join('?' for _ in chunk)})", chunk)
    s.db.log_event("info", "accounts", f"Removed account #{account_id} and {len(orphan_ids)} game(s)")
    return {"deleted": True, "games_removed": len(orphan_ids)}


@router.post("/accounts/{account_id}/sync")
def sync_account(request: Request, account_id: int):
    s = S(request)
    account_or_404(s, account_id)
    s.sync.request(account_id)
    return {"requested": True}


class AnalyzeScope(BaseModel):
    filters: dict = {}
    depth: int | None = Field(None, ge=6, le=30)


@router.post("/accounts/{account_id}/analyze")
def analyze_scope(request: Request, account_id: int, body: AnalyzeScope):
    s = S(request)
    account_or_404(s, account_id)
    ids = scope_game_ids(s.db, account_id, Filters.from_dict(body.filters))
    added = s.analysis.enqueue(ids, PRIORITY_REPORT, depth=body.depth)
    return {"scope": len(ids), "queued": added}
