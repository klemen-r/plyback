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
| Bound to a network interface (`--lan`, Docker, a server) | The server requires an access password at startup and while running, unless `PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE=1` is set explicitly. If the password is removed while the server is running, restore access with `plyback set-password`. |

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
- **Concurrency** — request work is bounded; password verification and rotation use a transactional
  comparison before creating a session. Revoked sessions are checked again after body upload.
  Background results require the original game identity and current job claim before they can commit;
  sync writes check immutable account generations so deleted IDs cannot receive stale responses.
- **Engine** — UCI output, deadlines and retries are bounded. Cancelled requests stop searching;
  unresponsive engines are killed and reaped. Worker count, threads and hash have aggregate limits.
- **Dependencies** — checked in CI with `cargo audit` and `npm audit`; Dependabot covers Rust,
  JavaScript and GitHub Actions. The production runtime contains no Python or Node.

## Running behind a reverse proxy

Terminate TLS at the proxy, forward to `127.0.0.1:8000`, and set `PLYBACK_SECURE_COOKIES=1`.
Preserve the public Host and list it in `PLYBACK_ALLOWED_HOSTS`, a comma-separated list of names
without schemes or ports. Host checks now apply with a password too: existing protected deployments
using DNS aliases or tailnet names must add them. Loopback names and the bind address are accepted;
with a password on a wildcard bind (`0.0.0.0` or `::`), OS interface IPs and the OS hostname are
also accepted. Discovery runs at startup; restart after interface/name changes or list stable aliases
explicitly. Request Host names are never resolved to decide access. The legacy `*` entry accepts any
Host and disables this DNS-rebinding defense.
For Docker's published port, add the host machine's IP/name explicitly: automatic discovery
sees the container's own interfaces and hostname.

Rate limits use the TCP peer by default, even for loopback peers. To keep clients from sharing a
proxy's bucket, set `PLYBACK_TRUSTED_PROXIES` to exact connecting proxy IPs, such as `127.0.0.1,::1`.
Names, CIDR ranges and wildcards are rejected at startup. Trust only controlled proxies; each must
overwrite incoming client-IP headers or append the actual peer correctly. Restrict direct backend
access accordingly. For multiple proxy hops, list every controlled hop that should be skipped.
Strip any unused client-IP header: a proxy that sets only `Forwarded` must remove incoming
`X-Forwarded-For`, since XFF takes precedence.

Only listed peers may supply `X-Forwarded-For` or RFC-style `Forwarded` client IPs. Chains are read
right to left, skipping listed proxies and stopping at the first untrusted IP. Repeated header fields
retain their order. `X-Forwarded-For` takes precedence over `Forwarded`; invalid preferred headers
never fall through to the other header. Missing/invalid chains use the TCP peer bucket. XFF accepts
literal IPv4/IPv6 addresses; `Forwarded` also accepts quoted IPs and IP/port pairs (brackets for IPv6).
Unknown/obfuscated nodes, duplicate `for` parameters, escaped quoted strings and chains over 64 hops
fall back to the peer. Delimiters in quoted metadata never create extra hops or parameters.
IPv4-mapped IPv6 addresses share their IPv4 bucket. This setting changes only rate-limit identity;
the existing loopback-only `X-Forwarded-Proto` handling for scheme/cookies is unchanged.

Passwordless Tailscale Serve remains supported: bind to loopback and set
`PLYBACK_ALLOWED_HOSTS=plyback.tailnet.example`. Forwarding headers are still accepted without
a password; everyone who can reach that shared tailnet URL has full API access. The trusted-proxy
setting is optional and does not enable or require authentication. Removing the access password
through the API is refused while configured to listen on a network interface.

An instance is shared by one person or household. It does not provide per-user tenant isolation.
Backups contain password hashes and sessions along with game data; protect backup files as private data.
