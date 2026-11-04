# Plyback engineering conventions

- Preserve the user's database, exports, features and design studies. Never run benchmarks or tests
  against writable live data; use SQLite backup snapshots and temporary directories.
- The frontend direction is A, Vercel tabs (chosen by the owner on 2026-11-03): breadcrumb header,
  underlined tabs, bordered cards and stat grids, weight 600, sentence case. Keep the ⌘K palette.
  Avoid uppercase eyebrows, pills, heavy bold, serif display mixes, sidebars and neon accents.
- Source-checkout and Docker are supported distributions. Do not claim a standalone pip wheel includes the UI.
- Append schema migrations. Keep schema/backfill/version changes atomic. Increment analysis version when
  stored results change. Maintain immutable report snapshots and per-database cache ownership.
- Profile real workloads before optimization. Preserve search budgets/quality or disclose explicit tradeoffs.
- Run backend Ruff and non-live pytest; frontend lint, tests, build and Playwright. Use isolated test data.
  CI must verify Linux, Windows and the container. Keep required third-party licenses/provenance.
- Use independent agents where useful for bounded audits and verification; coordinate file ownership.
