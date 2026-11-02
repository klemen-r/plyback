# Security verification — 2026-11-02

Reviewed the FastAPI API/middleware, authentication, SQLite access, outbound Chess.com client,
Stockfish installation, browser request handling, static delivery and deployment defaults.
This records verified repairs and their limits, not a guarantee that no vulnerabilities remain.

| Finding | Impact before repair | Repair and evidence |
|---|---|---|
| Header-only upload limit | Chunked or inaccurate-length uploads could bypass the stated memory limit | Native streamed byte limit; regression checks 413 and security headers without Content-Length |
| Incomplete origin comparison | Same host on a different port/scheme and opaque origins accepted | Exact scheme/host/port checks; cross-origin and IPv6 regressions |
| Unchecked HTTP redirects | A validated archive URL could redirect the server outside the API | Every outgoing request and redirect is checked against the configured origin/base path before transport |
| Unbounded list arguments | Negative LIMIT disabled SQLite pagination limits | Positive API bounds and HTTP validation regressions |
| Network password removal | Host headers were used to decide whether remote protection could be removed | API refuses removal on a configured non-loopback bind; regression tests forged localhost Host |
| Shared process caches | Separate database instances could reuse another database's result | Instance-owned result cache and database-owned move caches; cross-database regressions |
| Installer publication | Failed replacement could destroy a working engine and notices were discarded | Digest validation, bounded staged extraction, UCI verification, atomic publication, retained notices/source provenance |
| Startup/shutdown ownership | Failed startup or retired workers could outlive storage cleanup | Cleanup encloses startup; joined services/cache workers and released thread connections |

Authentication still requires the current password to change/remove it, invalidates all sessions on
change, stores only session token hashes, and uses HttpOnly/SameSite cookies. Default binding is loopback;
network launch requires an access password. SQL parameters are bound; dynamic identifiers are internal
whitelists or schema-derived. React escapes content; external links use noreferrer and the runtime
uses a restrictive CSP with self-hosted scripts/fonts. A development-only magic host exception was removed.

Tests are in `test_security.py`, `test_api_hardening.py`, `test_api_cache.py`, `test_database.py`,
`test_backup.py`, `test_services.py` and `test_installer.py`, with browser authentication regressions.
Locked Python production/development packages and all npm dependencies had no known advisories in the
local audits performed for this change. CI repeats those audits; advisory availability can change.

Operational boundaries remain: this is a shared household instance, not isolated user tenants;
TLS termination belongs to the deployment proxy; the explicit unauthenticated-remote override removes
password protection; backups contain sensitive hashes/sessions. Keep trusted proxies local and protect
both the data directory and backups. A full external penetration test was not performed.

Reference behavior was checked against the installed Starlette body-limit implementation and
[HTTPX redirect documentation](https://www.python-httpx.org/compatibility/#redirects).
