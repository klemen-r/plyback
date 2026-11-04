"""Refresh the public request schema from the preserved API compatibility oracle.

Development-only: the native server embeds the resulting JSON and never imports Python.
"""
from __future__ import annotations

import json
import logging
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "backend"))

with tempfile.TemporaryDirectory(prefix="plyback-api-contract-") as directory:
    os.environ["PLYBACK_DATA"] = directory
    from plyback.main import create_app

    app = create_app(Path(directory) / "contract.db", start_background=False, serve_frontend=False)
    schema = app.openapi()
    schema["info"]["version"] = "3.0.0"
    schema["info"]["description"] = (
        "Plyback native server. Password sessions and X-Requested-With: plyback protect API requests."
    )
    (ROOT / "rust" / "openapi.json").write_text(json.dumps(schema, indent=2) + "\n", encoding="utf-8")
    app.state.services.db.close_all()
    app.state.insight_cache.close()
    logging.shutdown()
