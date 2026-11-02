# Contributing

Plyback supports running from a source checkout or its Docker image. Python 3.10+ and Node.js 22.12+
are required. `setup.ps1` / `bash setup.sh` prepare the runtime; install development dependencies
with `python -m pip install --require-hashes -r backend/requirements-dev.txt` using the project venv.

Before submitting a change, run backend Ruff and `pytest -m "not live"`, then frontend lint, unit tests,
build and Playwright tests. The README contains platform-specific commands. Real Stockfish tests need
the official engine; `scripts/install_stockfish.py` installs it. Network tests are opt-in with `-m live`.
CI runs Linux/Windows checks, the browser workflows and a Docker authentication smoke test.

Keep fixes focused, preserve existing game data and public API behavior, and add regressions for
meaningful bugs. Schema changes append migrations; never modify a released migration. Keep each schema
change, backfill and version update atomic. Bump `ANALYSIS_VERSION` when stored analysis behavior changes.
Use SQLite's backup API for active databases and keep personal exports and database files out of commits.

Profile before optimizing. Report the workload, engine options, repeated measurements and output checks;
do not obtain speed claims by silently reducing engine depth or removing tactical analysis.

The final frontend direction is undecided: A, D, A+D and C are preserved under `docs/design-explorations`.
Use current shared primitives for functional changes and wait for explicit user selection before redesigning.
Retain copyright, dependency licenses and required notices. Report vulnerabilities privately as described
in SECURITY.md rather than opening public issues.
