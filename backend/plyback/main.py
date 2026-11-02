"""Application factory: wires services together and serves the API and the web UI."""

from __future__ import annotations

import logging
import logging.handlers
import os
import sqlite3
import tempfile
import threading
from contextlib import asynccontextmanager, closing
from dataclasses import dataclass
from pathlib import Path

from fastapi import FastAPI, Request
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles

from . import config, security
from .analysis import openings
from .chesscom.client import ChessComClient
from .db import Database
from .services.analysis import AnalysisService
from .services.interactive import InteractiveEngine
from .services.reports import ReportService
from .services.sync import SyncService

log = logging.getLogger("plyback")


def setup_logging(level: str | None = None) -> None:
    root = logging.getLogger()
    if getattr(root, "_plyback_configured", False):
        return
    root.setLevel(level or os.environ.get("PLYBACK_LOG_LEVEL", os.environ.get("LOG_LEVEL", "INFO")))
    fmt = logging.Formatter("%(asctime)s %(levelname)-7s %(name)s: %(message)s")
    stream = logging.StreamHandler()
    stream.setFormatter(fmt)
    root.addHandler(stream)
    try:
        (config.DATA_DIR / "logs").mkdir(parents=True, exist_ok=True)
        file = logging.handlers.RotatingFileHandler(
            config.DATA_DIR / "logs" / "plyback.log", maxBytes=5_000_000, backupCount=3, encoding="utf-8"
        )
        file.setFormatter(fmt)
        root.addHandler(file)
    except OSError:
        log.warning("File logging unavailable")
    for noisy in ("httpx", "chess.engine"):
        logging.getLogger(noisy).setLevel(logging.WARNING if noisy == "httpx" else logging.ERROR)
    root._plyback_configured = True  # type: ignore[attr-defined]


def backup_database(source: Path, destination: Path) -> None:
    """Publish a consistent SQLite snapshot without overwriting an existing file."""
    source, destination = source.resolve(), destination.resolve()
    if not source.is_file():
        raise FileNotFoundError(source)
    if destination.exists():
        raise FileExistsError(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=".plyback-backup-", suffix=".db", dir=destination.parent)
    os.close(fd)
    temporary = Path(name)
    try:
        with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as reader:
            writer = sqlite3.connect(temporary)
            try:
                reader.backup(writer)
                if writer.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                    raise RuntimeError("Database snapshot failed its integrity check")
            finally:
                writer.close()
        # Atomic create-if-absent, including on POSIX where rename would overwrite.
        os.link(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def adopt_legacy_database() -> None:
    """Snapshot a pre-2.0 database, retaining its original file and WAL for rollback."""
    if config.DB_PATH.exists() or not config.LEGACY_DB_PATH.exists():
        return
    try:
        backup_database(config.LEGACY_DB_PATH, config.DB_PATH)
    except FileExistsError:
        return
    log.info("Copied legacy database to %s; original retained", config.DB_PATH)


@dataclass
class Services:
    db: Database
    analysis: AnalysisService
    sync: SyncService
    reports: ReportService
    interactive: InteractiveEngine
    started: bool = False


def create_app(
    db_path: str | Path | None = None,
    start_background: bool = True,
    chesscom_client: ChessComClient | None = None,
    engine_factory=None,
    interactive_factory=None,
    serve_frontend: bool = True,
) -> FastAPI:
    setup_logging()
    if db_path is None:
        adopt_legacy_database()
    db = Database(db_path or config.DB_PATH)
    db.migrate()
    if config.INITIAL_PASSWORD and not security.auth_required(db):
        security.validate_new_password(config.INITIAL_PASSWORD)
        security.set_password(db, config.INITIAL_PASSWORD)
        log.info("Access password set from PLYBACK_PASSWORD")
    analysis = AnalysisService(db, engine_factory=engine_factory)
    sync = SyncService(db, analysis, client=chesscom_client)
    reports = ReportService(db, analysis)
    interactive = InteractiveEngine(interactive_factory)
    services = Services(db, analysis, sync, reports, interactive)

    from .api.insights import InsightCache

    insight_cache = InsightCache(cleanup=db.close_thread)

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        threading.Thread(target=openings.warm, name="openings-warm", daemon=True).start()
        try:
            if start_background:
                analysis.start()
                sync.start()
                reports.start()
                services.started = True
                log.info("Plyback %s started (data: %s)", config.APP_VERSION, config.DATA_DIR)
            yield
        finally:
            sync.stop()
            reports.stop()
            analysis.stop()
            insight_cache.close()
            interactive.close()
            db.close_all()

    docs = os.environ.get("PLYBACK_API_DOCS") == "1"
    app = FastAPI(
        title="Plyback",
        version=config.APP_VERSION,
        lifespan=lifespan,
        openapi_url="/api/openapi.json",
        docs_url="/api/docs" if docs else None,
        redoc_url=None,
    )
    app.state.services = services
    app.state.insight_cache = insight_cache
    security.install(app, lambda: db)

    @app.exception_handler(ValueError)
    async def value_error_handler(request: Request, exc: ValueError):
        return JSONResponse(status_code=400, content={"detail": str(exc)})

    from .api import accounts, auth, engine, games, insights, queue, system

    for module in (system, auth, accounts, games, engine, queue, insights):
        app.include_router(module.router, prefix="/api")

    @app.api_route("/api/{rest:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE"], include_in_schema=False)
    async def api_not_found(rest: str):
        return JSONResponse(status_code=404, content={"detail": "Not found"})

    dist = config.FRONTEND_DIST
    if serve_frontend and (dist / "index.html").exists():
        if (dist / "assets").exists():
            app.mount("/assets", StaticFiles(directory=dist / "assets"), name="assets")

        @app.get("/{full_path:path}", include_in_schema=False)
        async def spa(full_path: str):
            candidate = (dist / full_path).resolve()
            if full_path and candidate.is_file() and dist in candidate.parents:
                return FileResponse(candidate)
            return FileResponse(dist / "index.html", headers={"Cache-Control": "no-cache"})

    return app


def run() -> None:
    import uvicorn

    setup_logging()
    if not security.is_loopback_host(config.HOST) and not config.ALLOW_UNAUTHENTICATED_REMOTE:
        if config.DB_PATH.exists() or config.LEGACY_DB_PATH.exists():
            adopt_legacy_database()
        probe = Database(config.DB_PATH)
        probe.migrate()
        protected = security.auth_required(probe) or bool(config.INITIAL_PASSWORD)
        probe.close_all()
        if not protected:
            raise SystemExit(
                f"Refusing to listen on {config.HOST} without an access password.\n"
                "Set one with `python -m plyback set-password` or PLYBACK_PASSWORD, "
                "or set PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE=1 if the network is trusted."
            )
    uvicorn.run(
        create_app(),
        host=config.HOST,
        port=config.PORT,
        log_level="warning",
        server_header=False,
        proxy_headers=True,
        forwarded_allow_ips="127.0.0.1",
    )


if __name__ == "__main__":
    run()
