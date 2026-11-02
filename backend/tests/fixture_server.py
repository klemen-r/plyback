"""A tiny stand-in for the Chess.com published-data API, serving captured fixtures.

Used by the end-to-end tests:  python -m tests.fixture_server --port 8799
then start the app with CHESSCOM_API_BASE=http://127.0.0.1:8799/pub
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import uvicorn
from fastapi import FastAPI, Request, Response
from fastapi.responses import JSONResponse

FIX = Path(__file__).parent / "fixtures" / "chesscom"
app = FastAPI()


def load(name: str):
    return json.loads((FIX / name).read_text(encoding="utf-8"))


@app.get("/pub/player/{user}")
def profile(user: str):
    if user != "klemen_r":
        return JSONResponse({"message": "not found"}, status_code=404)
    return load("profile.json")


@app.get("/pub/player/{user}/stats")
def stats(user: str):
    return load("stats.json")


@app.get("/pub/player/{user}/games/archives")
def archives(user: str):
    return load("archives.json")


@app.get("/pub/player/{user}/games/{y}/{m}")
def month(user: str, y: str, m: str, request: Request):
    name = f"{y}-{m}.json"
    if not (FIX / name).exists():
        return {"games": []}
    etag = f'"{y}{m}"'
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304)
    return JSONResponse(load(name), headers={"ETag": etag})


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8799)
    uvicorn.run(app, host="127.0.0.1", port=ap.parse_args().port, log_level="warning")
