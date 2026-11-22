# Contributing

## Setup

Install Rust 1.97.1+ and Node.js 22.12+, then run `setup.ps1` (Windows) or `bash setup.sh`.
On macOS and Linux, `just setup-dev` also installs Chromium for the browser tests.

## Checks

Run all of these before submitting a change. On macOS and Linux, `just check` runs them in order.

```sh
node --test scripts/*.test.mjs

cargo fmt --manifest-path rust/Cargo.toml --check
cargo clippy --locked --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --release --locked --manifest-path rust/Cargo.toml

cd frontend
npm ci
npm run lint
npm test
npm run build
npx playwright install chromium
npm run e2e
```

Tests run against temporary data directories, never against `data/`. Real-engine tests need
Stockfish (`node scripts/install_stockfish.mjs`); CI sets `PLYBACK_REQUIRE_STOCKFISH=1` so they cannot
skip. CI runs on Linux and Windows and builds the container.

`just dev` serves the interface with live reload at localhost:5173; run `just run` alongside it.
Development-only settings, such as local API fixtures, are listed in
[docs/development.md](docs/development.md).

## Migrations and analysis versions

Schema changes go in a new file in `rust/migrations/`; never edit a released one. Keep a schema change,
its backfill and the version update in one migration. Bump `ANALYSIS_VERSION` when stored analysis
results change, so existing games are re-reviewed.

## Dependencies and licences

Rust uses `rust/Cargo.lock`; after changing it, regenerate `docs/licenses/rust.txt` with
`node scripts/rust_notices.mjs`. JavaScript uses `npm ci` and `package-lock.json`.
Review dependency and licence changes before committing, and keep the required notices.

## Releases

Set the new version in `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/openapi.json`,
`frontend/package.json` and `frontend/package-lock.json`, merge it, then push a tag such as `v3.1.0`.
The release workflow builds and smoke-tests the archives for each platform, publishes the container
image to `ghcr.io/klemen-r/plyback` and drafts a GitHub release with the archives and checksums.
Write the notes and publish the draft. Running the workflow by hand builds everything without
publishing.

## Interface

Build pages from the shared primitives in `frontend/src/components/ui.tsx`. The style is plain: a
breadcrumb header, underlined tabs, bordered cards and stat grids, weight 600 for emphasis and
sentence case throughout.

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).
