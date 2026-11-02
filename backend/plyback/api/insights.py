"""Account insights and saved reports."""

from __future__ import annotations

import logging
import threading
import time
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from ..db import dumps, loads
from ..insights.compute import Filters, compute_insights
from .deps import S, account_or_404

router = APIRouter()
log = logging.getLogger(__name__)


class InsightCache:
    """Bounded, instance-owned cache with coalesced background refreshes."""

    STALE_FOR = 120

    def __init__(self, size: int = 64, cleanup=lambda: None):
        self._data: OrderedDict = OrderedDict()
        self._pending: dict = {}
        self._lock = threading.Lock()
        self._size = size
        self._cleanup = cleanup
        self._executor = ThreadPoolExecutor(max_workers=2, thread_name_prefix="insights")

    def close(self):
        self._executor.shutdown(wait=True, cancel_futures=True)

    def get(self, key, fingerprint, compute):
        with self._lock:
            hit = self._data.get(key)
            if hit and hit[0] == fingerprint:
                self._data.move_to_end(key)
                return hit[1]
            pending = self._pending.get(key)
            if pending is None and len(self._pending) < self._size:
                token = object()
                future = self._executor.submit(self._compute, key, fingerprint, compute, token)
                self._pending[key] = (fingerprint, future, token)
            else:
                future = pending[1] if pending else None
            if hit and time.time() - hit[1]["generated_at"] < self.STALE_FOR:
                return {**hit[1], "stale": True}
        if future is None:
            return self._compute(key, fingerprint, compute)
        return future.result()

    def _compute(self, key, fingerprint, compute, token=None):
        try:
            data = compute()
            data["generated_at"] = int(time.time())
            with self._lock:
                self._data[key] = (fingerprint, data)
                self._data.move_to_end(key)
                while len(self._data) > self._size:
                    self._data.popitem(last=False)
            return data
        except Exception:
            log.exception("Insight refresh failed")
            raise
        finally:
            self._cleanup()
            with self._lock:
                pending = self._pending.get(key)
                if pending and pending[2] is token:
                    self._pending.pop(key)


def _fingerprint(s, account_id: int) -> int:
    return s.db.scalar("SELECT revision FROM data_revision")


@router.get("/accounts/{account_id}/insights")
def insights(
    request: Request,
    account_id: int,
    since: int | None = None,
    until: int | None = None,
    time_classes: str | None = None,
    color: str | None = None,
    rated: str | None = None,
    opening: str | None = None,
    opponent: str | None = None,
):
    s = S(request)
    account_or_404(s, account_id)
    f = Filters.from_dict(
        {
            "since": since,
            "until": until,
            "time_classes": time_classes,
            "color": color,
            "rated": rated,
            "opening": opening,
            "opponent": opponent,
        }
    )
    key = (account_id, dumps(f.to_dict()))
    return request.app.state.insight_cache.get(
        key, _fingerprint(s, account_id), lambda: compute_insights(s.db, account_id, f)
    )


class ReportIn(BaseModel):
    filters: dict = {}
    title: str | None = Field(None, max_length=120)


def _report_public(r: dict, full: bool = False) -> dict:
    out = {
        k: r[k]
        for k in (
            "id",
            "account_id",
            "title",
            "status",
            "total_games",
            "analyzed_games",
            "created_at",
            "completed_at",
            "error",
        )
    }
    out["filters"] = loads(r["filters_json"], {})
    if full:
        out["report"] = loads(r.get("report_json"), None)
    return out


@router.post("/accounts/{account_id}/reports", status_code=201)
def create_report(request: Request, account_id: int, body: ReportIn):
    s = S(request)
    account_or_404(s, account_id)
    filters = Filters.from_dict(body.filters).to_dict()
    title = body.title or _default_title(filters)
    rid = s.reports.create(account_id, filters, title)
    return _report_public(s.db.one("SELECT * FROM reports WHERE id=?", (rid,)))


def _default_title(filters: dict) -> str:
    parts = []
    if filters.get("time_classes"):
        tc = filters["time_classes"]
        parts.append((", ".join(tc) if isinstance(tc, list) else str(tc)).title())
    if filters.get("since") is not None:
        parts.append("since " + datetime.fromtimestamp(int(filters["since"]), timezone.utc).strftime("%b %Y"))
    else:
        parts.append("all time")
    if filters.get("color"):
        parts.append(f"as {filters['color']}")
    return "Account analysis — " + " ".join(parts)


@router.get("/accounts/{account_id}/reports")
def list_reports(request: Request, account_id: int):
    s = S(request)
    return [
        _report_public(r)
        for r in s.db.query("SELECT * FROM reports WHERE account_id=? ORDER BY created_at DESC", (account_id,))
    ]


@router.get("/reports/{report_id}")
def get_report(request: Request, report_id: int):
    s = S(request)
    r = s.db.one("SELECT * FROM reports WHERE id=?", (report_id,))
    if r is None:
        raise HTTPException(404, "Report not found")
    return _report_public(r, full=True)


@router.post("/reports/{report_id}/finalize")
def finalize_report(request: Request, report_id: int):
    s = S(request)
    if not s.db.one("SELECT id FROM reports WHERE id=?", (report_id,)):
        raise HTTPException(404, "Report not found")
    s.reports.finalize(report_id)
    return _report_public(s.db.one("SELECT * FROM reports WHERE id=?", (report_id,)))


@router.delete("/reports/{report_id}")
def delete_report(request: Request, report_id: int):
    S(request).db.execute("DELETE FROM reports WHERE id=?", (report_id,))
    return {"deleted": True}
