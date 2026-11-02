"""Analysis queue control."""

from __future__ import annotations

from fastapi import APIRouter, HTTPException, Request

from .deps import S

router = APIRouter()


@router.get("/queue")
def queue(request: Request):
    s = S(request)
    stats = s.analysis.stats()
    queued = s.db.query("""SELECT j.id, j.game_id, j.priority, j.depth, j.status, j.attempts, j.created_at,
                                  g.white_username, g.black_username, g.end_time, g.time_class
                           FROM jobs j JOIN games g ON g.id = j.game_id WHERE j.status IN ('queued','running')
                           ORDER BY j.status='running' DESC, j.priority DESC, j.sort_key DESC LIMIT 50""")
    failed = s.db.query("""SELECT j.id, j.game_id, j.error, j.attempts, j.finished_at, g.white_username, g.black_username
                           FROM jobs j JOIN games g ON g.id = j.game_id WHERE j.status='failed'
                           ORDER BY j.finished_at DESC LIMIT 50""")
    for j in queued:
        for a in stats["active"]:
            if a["job_id"] == j["id"]:
                j["progress"], j["total"] = a["done"], a["total"]
    return {**stats, "jobs": queued, "failed_jobs": failed}


@router.post("/queue/{action}")
def queue_action(request: Request, action: str):
    s = S(request)
    if action == "pause":
        s.analysis.pause()
    elif action == "resume":
        s.analysis.resume()
    elif action == "retry-failed":
        return {"retried": s.analysis.retry_failed()}
    elif action == "clear":
        return {"cleared": s.analysis.clear_queue()}
    else:
        raise HTTPException(404, "Unknown action")
    return {"paused": s.analysis.paused}
