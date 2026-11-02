"""Health, status, settings and the event log."""

from __future__ import annotations

import os

from fastapi import APIRouter, Query, Request

from ..analysis.engine import EngineNotFound, locate_stockfish
from ..config import ANALYSIS_VERSION, APP_VERSION, DEPTH_PRESETS
from ..services import store
from .deps import S

router = APIRouter()


@router.get("/health")
def health():
    return {"ok": True, "version": APP_VERSION}


@router.get("/status")
def status(request: Request):
    s = S(request)
    try:
        engine_path = str(locate_stockfish())
        engine_error = s.analysis.engine_error
    except EngineNotFound as exc:
        engine_path, engine_error = None, str(exc)
    from pathlib import Path

    db_file = Path(s.db.path)
    return {
        "version": APP_VERSION,
        "analysis_version": ANALYSIS_VERSION,
        "engine": {"name": s.analysis.engine_name, "path": engine_path, "error": engine_error},
        "queue": s.analysis.stats(),
        "sync": {"current_account": s.sync.current},
        "accounts": s.db.scalar("SELECT COUNT(*) FROM accounts"),
        "games": s.db.scalar("SELECT COUNT(*) FROM games"),
        "analyzed": s.db.scalar("SELECT COUNT(*) FROM game_analysis"),
        "db_size": db_file.stat().st_size if db_file.exists() else None,
        "depth_presets": DEPTH_PRESETS,
        "cpu_count": os.cpu_count(),
    }


@router.get("/settings")
def get_settings(request: Request):
    return store.get_settings(S(request).db)


@router.put("/settings")
def put_settings(request: Request, patch: dict):
    s = S(request)
    out = store.save_settings(s.db, patch)
    if patch.keys() & {"workers", "threads_per_worker", "hash_mb"}:
        s.analysis.apply_settings()
    return out


@router.get("/events")
def events(request: Request, limit: int = Query(100, ge=1, le=500), account_id: int | None = None):
    if account_id:
        return S(request).db.query("SELECT * FROM events WHERE account_id=? ORDER BY id DESC LIMIT ?", (account_id, limit))
    return S(request).db.query("SELECT * FROM events ORDER BY id DESC LIMIT ?", (limit,))
