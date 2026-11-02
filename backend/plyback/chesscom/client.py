"""Chess.com Published-Data API client.

* Serial requests with a minimum spacing (the API rate-limits parallel requests).
* Conditional requests (ETag / Last-Modified → 304) so polling is cheap.
* Retries with exponential backoff for 429/5xx/network errors, honouring Retry-After.
"""

from __future__ import annotations

import logging
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any
from urllib.parse import unquote

import httpx

from ..config import APP_VERSION, CHESSCOM_API_BASE

log = logging.getLogger(__name__)


class ChessComError(RuntimeError):
    def __init__(self, message: str, status: int | None = None):
        super().__init__(message)
        self.status = status


class PlayerNotFound(ChessComError):
    pass


@dataclass
class Response:
    status: int
    data: Any
    etag: str | None
    last_modified: str | None

    @property
    def not_modified(self) -> bool:
        return self.status == 304


def user_agent(contact: str = "") -> str:
    ua = f"Plyback/{APP_VERSION} (self-hosted game review; +https://github.com/klemen-r/plyback)"
    # Header values must stay on one line.
    contact = "".join(ch for ch in contact if ch.isprintable())[:120]
    return f"{ua} contact: {contact}" if contact else ua


class ChessComClient:
    def __init__(
        self,
        base_url: str = CHESSCOM_API_BASE,
        contact: str = "",
        min_interval: float = 0.35,
        max_retries: int = 5,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
        timeout: float = 30.0,
    ):
        self.base_url = base_url.rstrip("/")
        self._base = httpx.URL(self.base_url)
        self._cancel = threading.Event()
        self._client = httpx.Client(
            headers={"User-Agent": user_agent(contact), "Accept": "application/json"},
            timeout=timeout,
            transport=transport,
            follow_redirects=True,
            event_hooks={"request": [self._validate_request]},
        )
        self.min_interval = min_interval
        self.max_retries = max_retries
        self._sleep = sleep
        self._lock = threading.Lock()
        self._last = 0.0

    def _validate_request(self, request: httpx.Request) -> None:
        url = request.url
        path = unquote(url.path)
        if (
            (url.scheme, url.host, url.port) != (self._base.scheme, self._base.host, self._base.port)
            or url.userinfo
            or not path.startswith(self._base.path.rstrip("/") + "/")
            or ".." in path.split("/")
            or "\\" in path
            or "%" in path
        ):
            raise ChessComError("Refused a URL outside the configured Chess.com API")

    def cancel(self) -> None:
        self._cancel.set()

    def _wait(self, seconds: float) -> None:
        if self._sleep is time.sleep:
            self._cancel.wait(seconds)
        else:
            self._sleep(seconds)
        if self._cancel.is_set():
            raise ChessComError("Request cancelled")

    def close(self) -> None:
        self._client.close()

    def set_contact(self, contact: str) -> None:
        self._client.headers["User-Agent"] = user_agent(contact)

    def get(self, path_or_url: str, etag: str | None = None, last_modified: str | None = None) -> Response:
        url = path_or_url if path_or_url.startswith("http") else f"{self.base_url}/{path_or_url.lstrip('/')}"
        headers = {}
        if etag:
            headers["If-None-Match"] = etag
        if last_modified:
            headers["If-Modified-Since"] = last_modified
        delay = 2.0
        last_exc: Exception | None = None
        for attempt in range(self.max_retries + 1):
            with self._lock:
                if self._cancel.is_set():
                    raise ChessComError("Request cancelled")
                wait = self.min_interval - (time.monotonic() - self._last)
                if wait > 0:
                    self._wait(wait)
                try:
                    r = self._client.get(url, headers=headers)
                except httpx.HTTPError as exc:
                    r = None
                    last_exc = exc
                finally:
                    self._last = time.monotonic()
            if r is None:
                log.warning("Chess.com request failed (%s), retry %d/%d", last_exc, attempt + 1, self.max_retries)
            elif r.status_code == 304:
                return Response(304, None, etag, last_modified)
            elif r.status_code == 200:
                try:
                    data = r.json()
                except ValueError as exc:
                    last_exc = exc
                    log.warning("Invalid JSON from %s", url)
                else:
                    return Response(200, data, r.headers.get("ETag"), r.headers.get("Last-Modified"))
            elif r.status_code in (404, 410):
                raise PlayerNotFound(f"Not found: {url}", r.status_code)
            elif r.status_code == 429 or r.status_code >= 500:
                retry_after = r.headers.get("Retry-After")
                try:
                    delay = max(delay, float(retry_after)) if retry_after else delay
                except ValueError:
                    pass
                last_exc = ChessComError(f"HTTP {r.status_code} from Chess.com", r.status_code)
                log.warning("Chess.com HTTP %d for %s; backing off %.1fs", r.status_code, url, delay)
            else:
                raise ChessComError(f"HTTP {r.status_code} from Chess.com for {url}", r.status_code)
            if attempt < self.max_retries:
                self._wait(min(delay, 120))
                delay *= 2
        raise ChessComError(f"Chess.com unavailable after {self.max_retries + 1} attempts: {last_exc}")

    # --- typed endpoints -------------------------------------------------------------
    def profile(self, username: str) -> dict:
        return self.get(f"player/{username.lower()}").data

    def stats(self, username: str) -> dict:
        return self.get(f"player/{username.lower()}/stats").data

    def archives(self, username: str, etag: str | None = None) -> Response:
        return self.get(f"player/{username.lower()}/games/archives", etag=etag)

    def archive(self, url: str, etag: str | None = None, last_modified: str | None = None) -> Response:
        return self.get(url, etag=etag, last_modified=last_modified)
