# Contributing

Plyback supports a source checkout or its Docker image. Rust 1.97.1+ and Node.js 22.12+ build the app;
the native production server requires neither Python nor Node. `setup.ps1` / `bash setup.sh` prepare it.
Python 3.11+ and the locked `backend/requirements-dev.txt` are only needed for the preserved development
oracle, its regression suite and comparative benchmarks.

Before submitting a change, run Cargo format, Clippy and release tests, the Node installer tests,
baseline Ruff and `pytest -m "not live"`, then frontend lint, unit tests, build and Playwright tests.
The README contains commands. Real Stockfish tests need the official engine;
`node scripts/install_stockfish.mjs` installs it. CI sets `PLYBACK_REQUIRE_STOCKFISH=1` so native
engine tests cannot silently skip. Python network tests are opt-in with `-m live`.
CI runs Linux/Windows checks, browser workflows against Rust and a Docker authentication smoke test.

Keep fixes focused, preserve existing game data and public API behavior, and add regressions for
meaningful bugs. Schema changes append migrations; never modify a released migration. Keep each schema
change, backfill and version update atomic. Bump `ANALYSIS_VERSION` when stored analysis behavior changes.
Use SQLite's backup API for active databases and keep personal exports and database files out of commits.

Profile before optimizing. Report the workload, engine options, repeated measurements and output checks;
do not obtain speed claims by silently reducing engine depth or removing tactical analysis.

The frontend follows study A, Vercel tabs (chosen 2026-11-03); the other studies remain as history under
`docs/design-explorations`. Build pages from the shared primitives in `frontend/src/components/ui.tsx`.
Retain copyright, dependency licenses and required notices; regenerate `docs/licenses/rust.txt` with
`node scripts/rust_notices.mjs` after a Cargo dependency update. Report vulnerabilities privately as described
in SECURITY.md rather than opening public issues.
