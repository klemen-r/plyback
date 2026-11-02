"""Background synchronisation of Chess.com accounts.

Every account is polled on an interval (faster while the player is actively playing).
Past monthly archives are fetched once and marked complete; the current month is
re-fetched with ETag/If-Modified-Since so unchanged archives cost a 304.
"""

from __future__ import annotations

import logging
import threading
import time
from datetime import datetime, timezone

from ..chesscom.client import ChessComClient, ChessComError, PlayerNotFound
from ..db import Database, dumps
from . import store
from .analysis import PRIORITY_HISTORY, PRIORITY_NEW, AnalysisService

log = logging.getLogger(__name__)
PROFILE_REFRESH = 6 * 3600
ACTIVE_WINDOW = 30 * 60
OFFICIAL_BASE = "https://api.chess.com/pub"


def _period_from_url(url: str) -> str:
    parts = url.rstrip("/").split("/")
    return f"{parts[-2]}/{parts[-1]}"


def _month_is_closed(period: str, now: datetime) -> bool:
    """True when the month ended more than two days ago (late-finishing daily games settle)."""
    y, m = (int(x) for x in period.split("/"))
    ny, nm = (y + 1, 1) if m == 12 else (y, m + 1)
    month_end = datetime(ny, nm, 1, tzinfo=timezone.utc)
    return (now - month_end).total_seconds() > 2 * 86400


class SyncService:
    def __init__(self, db: Database, analysis: AnalysisService, client: ChessComClient | None = None):
        self.db = db
        self.analysis = analysis
        s = store.get_settings(db)
        self.client = client or ChessComClient(contact=s.get("contact", ""))
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._requested: set[int] = set()
        self._lock = threading.Lock()
        self._thread: threading.Thread | None = None
        self.current: int | None = None
        self.on_tick = None  # optional callback (reports service)

    def start(self) -> None:
        with self.db.tx() as c:
            c.execute("UPDATE accounts SET sync_status='idle' WHERE sync_status='syncing'")
        self._thread = threading.Thread(target=self._loop, name="sync", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()
        self.client.cancel()
        if self._thread:
            self._thread.join()
        self.client.close()

    def request(self, account_id: int) -> None:
        with self._lock:
            self._requested.add(account_id)
        self._wake.set()

    # --- loop ------------------------------------------------------------------------
    def _due(self, acc: dict, s: dict, now: float) -> bool:
        if not acc["sync_enabled"]:
            return False
        last = acc["last_sync_at"] or 0
        active = acc["last_new_game_at"] and now - acc["last_new_game_at"] < ACTIVE_WINDOW
        interval = s["active_sync_interval_seconds"] if active else s["sync_interval_seconds"]
        if acc["sync_status"] == "error":
            interval = max(interval, 600)  # back off accounts that keep failing
        return now - last >= interval

    def _loop(self) -> None:
        try:
            self._run_loop()
        finally:
            self.db.close_thread()

    def _run_loop(self) -> None:
        while not self._stop.is_set():
            try:
                s = store.get_settings(self.db)
                self.client.set_contact(s.get("contact", ""))
                now = time.time()
                with self._lock:
                    requested, self._requested = self._requested, set()
                for acc in self.db.query("SELECT * FROM accounts ORDER BY id"):
                    if self._stop.is_set():
                        break
                    if acc["id"] in requested or self._due(acc, s, now):
                        self.sync_account(acc["id"])
                if self.on_tick:
                    self.on_tick()
            except Exception:
                log.exception("Sync loop error")
            self._wake.wait(5)
            self._wake.clear()

    # --- one account -------------------------------------------------------------------
    def sync_account(self, account_id: int) -> dict:
        acc = store.account_by_id(self.db, account_id)
        if acc is None:
            return {"error": "unknown account"}
        username = acc["username"]
        self.current = account_id
        self.db.execute("UPDATE accounts SET sync_status='syncing', sync_error=NULL WHERE id=?", (account_id,))
        initial = not acc["history_complete"]
        interrupted = False
        summary = {"new_games": 0, "archives_fetched": 0}
        try:
            now = time.time()
            if not acc["profile_fetched_at"] or now - acc["profile_fetched_at"] > PROFILE_REFRESH:
                self._refresh_profile(acc)
            arch = self.client.archives(username)
            urls = (arch.data or {}).get("archives", [])
            periods = [(_period_from_url(u), u) for u in urls]
            if acc["import_since"]:
                periods = [(p, u) for p, u in periods if p >= acc["import_since"]]
            existing = {r["period"]: r for r in self.db.query("SELECT * FROM archives WHERE account_id=?", (account_id,))}
            utc_now = datetime.now(timezone.utc)
            for period, url in sorted(periods, reverse=True):  # newest first
                if self._stop.is_set():
                    interrupted = True
                    break
                row = existing.get(period)
                if row and row["complete"]:
                    continue
                if self.client.base_url != OFFICIAL_BASE and url.startswith(OFFICIAL_BASE):
                    url = self.client.base_url + url[len(OFFICIAL_BASE) :]
                if not url.startswith(self.client.base_url + "/"):
                    log.warning("Ignoring archive URL outside the API: %s", url)
                    continue
                resp = self.client.archive(
                    url, etag=row["etag"] if row else None, last_modified=row["last_modified"] if row else None
                )
                complete = 1 if _month_is_closed(period, utc_now) else 0
                if resp.not_modified:
                    self.db.execute(
                        "UPDATE archives SET fetched_at=?, complete=? WHERE account_id=? AND period=?",
                        (int(now), complete, account_id, period),
                    )
                    continue
                summary["archives_fetched"] += 1
                games = (resp.data or {}).get("games", [])
                rows = [r for r in (store.game_row_from_chesscom(g) for g in games) if r]
                with self.db.tx() as c:
                    results = store.upsert_games(c, rows)
                    ids = [gid for gid, _ in results]
                    store.link_account_games(c, account_id, username, rows, ids)
                    c.execute(
                        """INSERT INTO archives (account_id, period, url, etag, last_modified, fetched_at,
                                                       game_count, complete)
                                 VALUES (?,?,?,?,?,?,?,?)
                                 ON CONFLICT(account_id, period) DO UPDATE SET etag=excluded.etag,
                                 last_modified=excluded.last_modified, fetched_at=excluded.fetched_at,
                                 game_count=excluded.game_count, complete=excluded.complete""",
                        (account_id, period, url, resp.etag, resp.last_modified, int(now), len(rows), complete),
                    )
                new_ids = [gid for gid, is_new in results if is_new]
                linked_new = self._linked_to_account(account_id, new_ids)
                summary["new_games"] += len(linked_new)
                if linked_new:
                    if initial:
                        if acc["analyze_history"]:
                            self.analysis.enqueue(linked_new, PRIORITY_HISTORY)
                    elif acc["auto_analyze"]:
                        self.analysis.enqueue(linked_new, PRIORITY_NEW)
            updates = {"last_sync_at": int(time.time()), "sync_status": "idle", "sync_error": None}
            if not interrupted:
                updates["history_complete"] = 1
            if summary["new_games"] and not initial:
                updates["last_new_game_at"] = int(time.time())
                self.db.log_event("info", "sync", f"{summary['new_games']} new game(s) for {username}", account_id)
            elif initial:
                self.db.log_event("info", "sync", f"Imported {summary['new_games']} game(s) for {username}", account_id)
            self._update(account_id, updates)
            if acc["analyze_history"]:
                self.backfill(account_id)
        except PlayerNotFound:
            self._update(
                account_id,
                {
                    "sync_status": "error",
                    "sync_error": f"Chess.com player '{username}' not found",
                    "last_sync_at": int(time.time()),
                },
            )
            self.db.log_event("error", "sync", f"Player {username} not found on Chess.com", account_id)
            summary["error"] = "not_found"
        except ChessComError as exc:
            self._update(account_id, {"sync_status": "error", "sync_error": str(exc), "last_sync_at": int(time.time())})
            self.db.log_event("warning", "sync", f"Sync of {username} failed: {exc}", account_id)
            summary["error"] = str(exc)
        except Exception as exc:
            log.exception("Unexpected sync failure for %s", username)
            self._update(
                account_id,
                {"sync_status": "error", "sync_error": f"Internal error: {exc}", "last_sync_at": int(time.time())},
            )
            summary["error"] = str(exc)
        finally:
            self.current = None
        return summary

    def backfill(self, account_id: int) -> int:
        """Queue every game of the account that has neither an analysis nor any job."""
        ids = [
            r["game_id"]
            for r in self.db.query(
                """SELECT ag.game_id FROM account_games ag JOIN games g ON g.id = ag.game_id
               LEFT JOIN game_analysis ga ON ga.game_id = ag.game_id
               LEFT JOIN jobs j ON j.game_id = ag.game_id
               WHERE ag.account_id = ? AND ga.game_id IS NULL AND j.id IS NULL
                 AND g.rules IN ('chess','chess960')""",
                (account_id,),
            )
        ]
        return self.analysis.enqueue(ids, PRIORITY_HISTORY) if ids else 0

    def _linked_to_account(self, account_id: int, game_ids: list[int]) -> list[int]:
        if not game_ids:
            return []
        q = ",".join("?" for _ in game_ids)
        return [
            r["game_id"]
            for r in self.db.query(
                f"SELECT game_id FROM account_games WHERE account_id=? AND game_id IN ({q})", [account_id, *game_ids]
            )
        ]

    def _update(self, account_id: int, fields: dict) -> None:
        sets = ", ".join(f"{k}=?" for k in fields)
        self.db.execute(f"UPDATE accounts SET {sets} WHERE id=?", (*fields.values(), account_id))

    def _refresh_profile(self, acc: dict) -> None:
        try:
            profile = self.client.profile(acc["username"])
            stats = self.client.stats(acc["username"])
        except PlayerNotFound:
            raise
        except ChessComError as exc:
            log.warning("Profile refresh failed for %s: %s", acc["username"], exc)
            return
        self._update(
            acc["id"],
            {
                "display_name": profile.get("username") or acc["username"],
                "avatar": profile.get("avatar"),
                "profile_json": dumps(profile),
                "stats_json": dumps(stats),
                "profile_fetched_at": int(time.time()),
            },
        )


def validate_username(client: ChessComClient, username: str) -> dict:
    """Fetch the profile (raises PlayerNotFound)."""
    return client.profile(username)
