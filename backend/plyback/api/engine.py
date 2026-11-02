"""Interactive engine analysis."""

from __future__ import annotations

import asyncio
import threading

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from ..analysis.engine import AnalysisCancelled, EngineNotFound
from ..services import store
from .deps import S

router = APIRouter()


class EngineIn(BaseModel):
    fen: str = Field(..., max_length=256)
    depth: int | None = Field(None, ge=6, le=32)
    multipv: int = Field(3, ge=1, le=5)


@router.post("/engine/analyse")
async def engine_analyse(request: Request, body: EngineIn):
    s = S(request)
    depth = body.depth or int(store.get_settings(s.db)["interactive_depth"])
    cancel = threading.Event()
    work = asyncio.create_task(asyncio.to_thread(s.interactive.analyse, body.fen, depth, body.multipv, cancel=cancel))
    try:
        while not work.done():
            if await request.is_disconnected():
                cancel.set()
                break
            await asyncio.sleep(0.025)
        return await asyncio.shield(work)
    except asyncio.CancelledError:
        cancel.set()
        try:
            await asyncio.shield(work)
        except AnalysisCancelled:
            pass
        raise
    except AnalysisCancelled as exc:
        raise HTTPException(499, "Analysis request was cancelled") from exc
    except EngineNotFound as exc:
        raise HTTPException(503, str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(400, f"Invalid position: {exc}") from exc
    finally:
        cancel.set()
