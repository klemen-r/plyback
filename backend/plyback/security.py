"""Authentication, request hardening and security headers.

Model: a Plyback instance belongs to one person or household. On the local machine it
is open by default; once a password is set (or the server listens on a network
interface, which requires one), every API call needs a session cookie.

Defences, in request order:
  * Host-header allow-list while unauthenticated access is possible (DNS rebinding).
  * Request body size limit.
  * CSRF: state-changing requests must carry ``X-Requested-With: plyback`` (a custom
    header forces a CORS pre-flight, which cross-site pages cannot pass) and, if an
    Origin header is present, it must match the Host.
  * Session authentication (HttpOnly, SameSite=Strict cookie; tokens stored hashed).
  * Per-client rate limits on expensive or sensitive endpoints.
  * Strict Content-Security-Policy and related headers on every response.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import ipaddress
import secrets
import threading
import time
from collections.abc import Awaitable, Callable
from urllib.parse import urlsplit

from fastapi import Request, Response
from fastapi.responses import JSONResponse
from starlette.middleware.body_limit import RequestBodyLimitMiddleware

from . import config
from .db import Database

SESSION_COOKIE = "plyback_session"
SESSION_TTL = 30 * 86400
PASSWORD_KEY = "auth_password_hash"  # noqa: S105 - settings key name
MIN_PASSWORD_LENGTH = 8
CSRF_HEADER = "x-requested-with"
CSRF_VALUE = "plyback"
LOOPBACK_NAMES = {"localhost", "127.0.0.1", "::1"}
PUBLIC_API = {"/api/health", "/api/auth/session", "/api/auth/login"}

CSP = "; ".join(
    [
        "default-src 'self'",
        "script-src 'self'",
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: https://images.chesscomfiles.com https://www.chess.com",
        "font-src 'self' data:",
        "connect-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'self'",
        "frame-ancestors 'none'",
    ]
)


# --- passwords -------------------------------------------------------------------------------
def hash_password(password: str) -> str:
    salt = secrets.token_bytes(16)
    n, r, p = 2**14, 8, 1
    digest = hashlib.scrypt(password.encode(), salt=salt, n=n, r=r, p=p, dklen=32)
    return f"scrypt${n}${r}${p}${base64.b64encode(salt).decode()}${base64.b64encode(digest).decode()}"


def verify_password(password: str, stored: str) -> bool:
    try:
        algo, n, r, p, salt, digest = stored.split("$")
        if algo != "scrypt":
            return False
        calc = hashlib.scrypt(password.encode(), salt=base64.b64decode(salt), n=int(n), r=int(r), p=int(p), dklen=32)
        return hmac.compare_digest(calc, base64.b64decode(digest))
    except (ValueError, TypeError):
        return False


def password_hash(db: Database) -> str | None:
    row = db.one("SELECT value FROM settings WHERE key=?", (PASSWORD_KEY,))
    return row["value"] if row else None


def set_password(db: Database, password: str | None) -> None:
    with db.tx() as c:
        if password is None:
            c.execute("DELETE FROM settings WHERE key=?", (PASSWORD_KEY,))
        else:
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                (PASSWORD_KEY, hash_password(password)),
            )
        c.execute("DELETE FROM sessions")


def validate_new_password(password: str) -> None:
    if len(password) < MIN_PASSWORD_LENGTH:
        raise ValueError(f"Use at least {MIN_PASSWORD_LENGTH} characters.")
    if len(password) > 256:
        raise ValueError("That password is too long.")


# --- sessions --------------------------------------------------------------------------------
def _token_hash(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def create_session(db: Database, user_agent: str | None) -> str:
    token = secrets.token_urlsafe(32)
    now = int(time.time())
    with db.tx() as c:
        c.execute("DELETE FROM sessions WHERE expires_at < ?", (now,))
        c.execute(
            "INSERT INTO sessions (token_hash, created_at, expires_at, last_seen, user_agent) VALUES (?,?,?,?,?)",
            (_token_hash(token), now, now + SESSION_TTL, now, (user_agent or "")[:200]),
        )
    return token


def session_valid(db: Database, token: str | None) -> bool:
    if not token or len(token) > 100:
        return False
    row = db.one("SELECT expires_at, last_seen FROM sessions WHERE token_hash=?", (_token_hash(token),))
    now = int(time.time())
    if not row or row["expires_at"] < now:
        return False
    if now - row["last_seen"] > 300:
        db.execute(
            "UPDATE sessions SET last_seen=?, expires_at=? WHERE token_hash=?", (now, now + SESSION_TTL, _token_hash(token))
        )
    return True


def end_session(db: Database, token: str | None) -> None:
    if token:
        db.execute("DELETE FROM sessions WHERE token_hash=?", (_token_hash(token),))


def set_session_cookie(response: Response, request: Request, token: str) -> None:
    response.set_cookie(
        SESSION_COOKIE,
        token,
        max_age=SESSION_TTL,
        httponly=True,
        samesite="strict",
        path="/",
        secure=config.SECURE_COOKIES or request.url.scheme == "https",
    )


def is_loopback_host(host: str) -> bool:
    host = host.strip("[]").lower()
    if host in LOOPBACK_NAMES or host.endswith(".localhost"):
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def auth_required(db: Database) -> bool:
    return password_hash(db) is not None


# --- rate limiting ---------------------------------------------------------------------------
class RateLimiter:
    """Token buckets keyed by (client, rule)."""

    def __init__(self) -> None:
        self._buckets: dict[tuple[str, str], tuple[float, float]] = {}
        self._lock = threading.Lock()

    def allow(self, client: str, rule: str, per_minute: int) -> bool:
        now = time.monotonic()
        rate = per_minute / 60.0
        with self._lock:
            tokens, last = self._buckets.get((client, rule), (float(per_minute), now))
            tokens = min(float(per_minute), tokens + (now - last) * rate)
            if tokens < 1:
                self._buckets[(client, rule)] = (tokens, now)
                return False
            self._buckets[(client, rule)] = (tokens - 1, now)
            if len(self._buckets) > 10_000:
                self._buckets.clear()
            return True


# (method, path prefix) -> requests per minute per client
RATE_RULES: list[tuple[str, str, int]] = [
    ("POST", "/api/auth/login", 10),
    ("POST", "/api/auth/password", 10),
    ("POST", "/api/engine/", 90),
    ("POST", "/api/games/import", 20),
    ("POST", "/api/accounts", 30),
    ("GET", "/api/accounts/", 240),
]


def _host_of(value: str) -> str:
    try:
        return urlsplit(value if "://" in value else "//" + value).hostname or ""
    except ValueError:
        return ""


def _origin(value: str) -> tuple | None:
    try:
        u = urlsplit(value)
        if u.scheme not in ("http", "https") or not u.hostname or u.username or u.password:
            return None
        if u.path not in ("", "/") or u.query or u.fragment:
            return None
        port = u.port if u.port is not None else (443 if u.scheme == "https" else 80)
        return u.scheme, u.hostname.lower(), port
    except ValueError:
        return None


def install(app, db_getter: Callable[[], Database]) -> None:
    """Register the security middleware on a FastAPI app."""
    limiter = RateLimiter()
    # Count received bytes as well as Content-Length, including chunked uploads.
    app.add_middleware(RequestBodyLimitMiddleware, max_body_size=config.MAX_BODY_BYTES)

    @app.middleware("http")
    async def security_middleware(request: Request, call_next: Callable[[Request], Awaitable[Response]]):
        db = db_getter()
        path = request.url.path
        method = request.method
        host_header = request.headers.get("host", "")
        host = _host_of(host_header).lower()
        needs_auth = auth_required(db)

        # 1. Host allow-list while the instance is open (prevents DNS-rebinding attacks).
        if not needs_auth and "*" not in config.ALLOWED_HOSTS:
            if not (is_loopback_host(host) or host in config.ALLOWED_HOSTS):
                return _deny(
                    400, "Unknown host. Set PLYBACK_ALLOWED_HOSTS or a password to allow access from other machines."
                )

        # 2. Body size.
        length = request.headers.get("content-length")
        if length and length.isdigit() and int(length) > config.MAX_BODY_BYTES:
            return _deny(413, "Request is too large.")

        if path.startswith("/api/"):
            # 3. CSRF.
            if method not in ("GET", "HEAD", "OPTIONS"):
                if request.headers.get(CSRF_HEADER, "").lower() != CSRF_VALUE:
                    return _deny(403, "Missing request header.")
                origin = request.headers.get("origin")
                if origin and (_origin(origin) is None or _origin(origin) != _origin(f"{request.url.scheme}://{host_header}")):
                    return _deny(403, "Cross-origin request refused.")
            # 4. Authentication.
            if needs_auth and path not in PUBLIC_API and not session_valid(db, request.cookies.get(SESSION_COOKIE)):
                return _deny(401, "Sign in to continue.")
            # 5. Rate limits.
            client = request.client.host if request.client else "?"
            for m, prefix, per_min in RATE_RULES:
                if method == m and path.startswith(prefix) and not limiter.allow(client, prefix, per_min):
                    return _deny(429, "Too many requests. Try again in a minute.", {"Retry-After": "60"})

        response = await call_next(request)
        _secure_headers(response, path)
        return response


def _deny(status: int, detail: str, headers: dict | None = None) -> Response:
    r = JSONResponse({"detail": detail}, status_code=status, headers=headers)
    _secure_headers(r, "/api/")
    return r


def _secure_headers(response: Response, path: str) -> None:
    h = response.headers
    h.setdefault("Content-Security-Policy", CSP)
    h.setdefault("X-Content-Type-Options", "nosniff")
    h.setdefault("X-Frame-Options", "DENY")
    h.setdefault("Referrer-Policy", "strict-origin-when-cross-origin")
    h.setdefault("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=()")
    h.setdefault("Cross-Origin-Opener-Policy", "same-origin")
    h.setdefault("Cross-Origin-Resource-Policy", "same-origin")
    if path.startswith("/api/"):
        h["Cache-Control"] = "no-store"
    elif path.startswith("/assets/"):
        h.setdefault("Cache-Control", "public, max-age=31536000, immutable")
