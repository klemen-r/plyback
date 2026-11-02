# Security

## Reporting a vulnerability

Report security issues privately to the repository owner. Where available, use GitHub's
[private vulnerability reporting](https://github.com/klemen-r/plyback/security/advisories/new).
For this private repository, use a private channel available to its collaborators.
Include steps to reproduce and the version you tested; do not publish exploit details in a public issue.

## Threat model

Plyback is a self-hosted application that holds one person's (or household's) chess data.
It stores public Chess.com game data and its own analyses; it never asks for Chess.com
credentials.

| Deployment | Protection |
|---|---|
| Local only (default, `127.0.0.1`) | Open without a password. Requests must carry a loopback `Host` header, which blocks DNS-rebinding attacks from web pages. |
| Reachable from other machines (`--lan`, Docker, a server) | An access password is **required**; the server refuses to start without one unless `PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE=1` is set explicitly. |

## Controls

- **Authentication** — password hashed with scrypt (N=2¹⁴, r=8, p=1, 16-byte salt). Sessions use
  256-bit random tokens stored only as SHA-256 hashes, in `HttpOnly`, `SameSite=Strict` cookies
  (`Secure` behind HTTPS or with `PLYBACK_SECURE_COOKIES=1`). Changing the password signs out every
  session. Login attempts are rate-limited per client.
- **CSRF** — state-changing API requests must send `X-Requested-With: plyback` (forcing a CORS
  pre-flight that cross-site pages cannot pass) and a matching `Origin` (scheme, host and port) when one is present. Opaque `null` origins are rejected.
- **Headers** — strict Content-Security-Policy (`script-src 'self'`, no inline scripts, no third-party
  origins except Chess.com avatar images), `frame-ancestors 'none'`, `nosniff`, a restrictive
  `Permissions-Policy`, and `Cache-Control: no-store` on API responses. Fonts and assets are
  self-hosted; the browser contacts no third party except for Chess.com avatars.
- **Input limits** — 25 MB request bodies enforced against actual streamed bytes as well as headers, validated usernames, FEN and depth bounds, rate limits on
  engine analysis and imports.
- **Outbound requests** — only to the Chess.com API base URL; archive URLs returned by the API are
  checked against it before being fetched, including every redirect destination.
- **Data at rest** — a single SQLite file in the data directory. Protect it with normal file
  permissions or disk encryption; the Docker image runs as an unprivileged user on a read-only root
  filesystem.
- **Dependencies** — audited in CI with `pip-audit` and `npm audit`; Dependabot keeps them current.

## Running behind a reverse proxy

Terminate TLS at the proxy, forward to `127.0.0.1:8000`, and set `PLYBACK_SECURE_COOKIES=1`.
Set `PLYBACK_ALLOWED_HOSTS` only if you need password-less access from other trusted host names.
Keep proxy headers restricted to a trusted local proxy. Removing the access password through the API
is refused while configured to listen on a network interface.

An instance is shared by one person or household. It does not provide per-user tenant isolation.
Backups contain password hashes and sessions along with game data; protect backup files as private data.
