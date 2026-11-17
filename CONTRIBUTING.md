# Contributing

## Setup

Install Rust 1.97.1+ and Node.js 22.12+, then run `setup.ps1` (Windows) or `bash setup.sh`. For the
Python baseline tests and benchmarks, also install Python 3.11+ with the locked
`backend/requirements-dev.txt`. On macOS/Linux, `just setup-dev` does all of this.

## Checks

Run these before submitting a change; the exact commands are in the README under
"Development and verification" (`just check` runs them on macOS/Linux):

- Cargo format, Clippy and release tests
- `node --test scripts/install_stockfish.test.mjs`
- Ruff and `pytest -m "not live"` in `backend/`
- frontend lint, unit tests, build and Playwright

Real-engine tests need Stockfish (`node scripts/install_stockfish.mjs`). CI sets
`PLYBACK_REQUIRE_STOCKFISH=1` so they cannot skip silently.

## Migrations

Schema changes go in a new file in `rust/migrations/`; never edit a released one. Keep the schema change,
its backfill and the version update in one migration. Bump `ANALYSIS_VERSION` when stored analysis
results change.

## Licenses

Keep copyright, dependency licenses and required notices. After a Cargo dependency update, regenerate
`docs/licenses/rust.txt` with `node scripts/rust_notices.mjs`.

## Interface

The frontend follows study A, Vercel tabs (chosen 2026-11-03); the other studies remain as history under
`docs/design-explorations`. Build pages from the shared primitives in `frontend/src/components/ui.tsx`.

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).
