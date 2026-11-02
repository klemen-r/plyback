"""Start a throw-away instance of the app for end-to-end tests.

Fresh data directory, Chess.com replaced by the local fixture server, shallow engine
depth so the full flow (sync → analysis → review → insights) completes in seconds.
"""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "backend" / "tests" / ".e2edata"
shutil.rmtree(DATA, ignore_errors=True)
DATA.mkdir(parents=True)
path_file = ROOT / "data" / "engine" / "stockfish.path"
if "PLYBACK_STOCKFISH" not in os.environ and path_file.exists():
    os.environ["PLYBACK_STOCKFISH"] = path_file.read_text().strip()
os.environ["PLYBACK_DATA"] = str(DATA)
os.environ.setdefault("PLYBACK_CHESSCOM_API", "http://127.0.0.1:8799/pub")
sys.path.insert(0, str(ROOT / "backend"))

import uvicorn  # noqa: E402

from plyback.db import Database  # noqa: E402
from plyback.main import create_app  # noqa: E402
from plyback.services import store  # noqa: E402

db = Database(DATA / "chess-insight.db")
db.migrate()
store.save_settings(db, {"analysis_depth": 10, "workers": 3, "max_seconds_per_position": 1.0, "interactive_depth": 12})
db.close_all()
uvicorn.run(create_app(), host="127.0.0.1", port=int(os.environ.get("PLYBACK_PORT", "8790")), log_level="warning")
