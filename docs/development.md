# Development settings

The API schema is available at `/api/openapi.json` under the same authentication
rules as other API endpoints. `PLYBACK_API_DOCS=1` additionally makes `/api/docs`
redirect to that schema.

Provider API overrides are intended for development and isolated tests:

- `PLYBACK_CHESSCOM_API` replaces the Chess.com API base (legacy alias:
  `CHESSCOM_API_BASE`). The client accepts HTTP or HTTPS without URL credentials,
  query parameters or fragments. Archive requests and redirects must remain
  within the configured scheme, host, port and base path.
- `PLYBACK_LICHESS_API` accepts `https://lichess.org` or an HTTP loopback server
  without credentials, a query, a fragment or a path prefix. Redirects are
  disabled. Use a local fixture server with temporary data when testing.
- `PLYBACK_NO_BACKGROUND=1` disables automatic sync, queue workers and report
  processing when diagnosing the server; it does not bypass authentication.
- `PLYBACK_RECONFIGURATION_CHILD` coordinates a subprocess within the Rust test
  suite. It is compiled only for tests and has no effect on the shipped server.

`PLYBACK_LOG_LEVEL` controls diagnostic logging. Review timing logs contain
elapsed times, counts and internal IDs, not HTTP request bodies or credentials.
Network binds always require a password, including development runs.

To measure review throughput or page performance, see [performance and measurement](app-performance.md).
