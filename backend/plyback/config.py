"""Configuration from the environment, and defaults for user-editable settings.

Every variable is prefixed ``PLYBACK_``; the pre-2.0 ``CHESS_INSIGHT_*`` names are still read.
"""

from __future__ import annotations

import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def env(name: str, default: str | None = None, legacy: str | None = None) -> str | None:
    value = os.environ.get(f"PLYBACK_{name}")
    if value is None and legacy:
        value = os.environ.get(legacy)
    return default if value in (None, "") else value


def env_flag(name: str) -> bool:
    return (env(name) or "").strip().lower() in ("1", "true", "yes", "on")


DATA_DIR = Path(env("DATA", str(ROOT / "data"), "CHESS_INSIGHT_DATA")).resolve()
DB_PATH = DATA_DIR / "plyback.db"
LEGACY_DB_PATH = DATA_DIR / "chess-insight.db"
FRONTEND_DIST = Path(env("FRONTEND", str(ROOT / "frontend" / "dist"), "CHESS_INSIGHT_FRONTEND")).resolve()
CHESSCOM_API_BASE = (env("CHESSCOM_API", None, "CHESSCOM_API_BASE") or "https://api.chess.com/pub").rstrip("/")
HOST = env("HOST", None, "HOST") or "127.0.0.1"
PORT = int(env("PORT", None, "PORT") or 8000)
# Optional: set the access password from the environment (e.g. in Docker).
INITIAL_PASSWORD = env("PASSWORD")
# Extra host names this server may be reached at (comma separated); "*" disables the check.
ALLOWED_HOSTS = [h.strip().lower() for h in (env("ALLOWED_HOSTS") or "").split(",") if h.strip()]
SECURE_COOKIES = env_flag("SECURE_COOKIES")
ALLOW_UNAUTHENTICATED_REMOTE = env_flag("ALLOW_UNAUTHENTICATED_REMOTE")
MAX_BODY_BYTES = 25 * 1024 * 1024
APP_VERSION = "2.0.0"
# Bump when classification/explanation logic changes so stale analyses can be detected.
ANALYSIS_VERSION = 6

DEPTH_PRESETS = {"fast": 12, "standard": 16, "deep": 20}

_cpus = os.cpu_count() or 2

DEFAULT_SETTINGS: dict = {
    # Engine
    "analysis_depth": 16,
    "analysis_multipv": 2,
    "max_seconds_per_position": 5.0,
    "workers": max(1, min(8, _cpus // 2)),
    "threads_per_worker": 1,
    "hash_mb": 64,
    "interactive_depth": 20,
    # Sync
    "sync_interval_seconds": 180,
    "active_sync_interval_seconds": 60,
    "auto_analyze_new": True,
    "analyze_history": True,
    "contact": "",
}

SETTING_LIMITS = {
    "analysis_depth": (6, 30),
    "analysis_multipv": (1, 5),
    "max_seconds_per_position": (0.2, 60.0),
    "workers": (1, max(1, _cpus)),
    "threads_per_worker": (1, max(1, _cpus)),
    "hash_mb": (16, 4096),
    "interactive_depth": (8, 32),
    "sync_interval_seconds": (60, 86400),
    "active_sync_interval_seconds": (30, 3600),
}
