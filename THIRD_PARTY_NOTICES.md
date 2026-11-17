# Third-party notices

Plyback is distributed under GPL-3.0-or-later. Third-party components retain their own copyright
and license terms. Dependency source links and complete web dependency notices are preserved below.

| Component | Source | License |
|---|---|---|
| shakmaty / pgn-reader | https://github.com/niklasf/shakmaty / https://github.com/niklasf/rust-pgn-reader | GPL-3.0-or-later |
| Axum / Tokio / Tower | https://github.com/tokio-rs | MIT |
| rusqlite / bundled SQLite | https://github.com/rusqlite/rusqlite / https://sqlite.org | MIT / public domain |
| Rust dependency tree | Exact package versions and source links in `rust/Cargo.lock` and the notice bundle | Individual terms in `docs/licenses/rust.txt` |
| @lichess-org/chessground and its bundled piece assets | https://github.com/lichess-org/chessground | GPL-3.0-or-later |
| Stockfish (separately downloaded) | https://github.com/official-stockfish/Stockfish | GPL-3.0 |
| Lichess opening dataset (`rust/data/openings/`) | https://github.com/lichess-org/chess-openings | CC0-1.0 |
| chess.js | https://github.com/jhlywa/chess.js | BSD-2-Clause |
| React / React DOM | https://github.com/facebook/react | MIT |
| React Router | https://github.com/remix-run/react-router | MIT |
| TanStack Query | https://github.com/TanStack/query | MIT |
| Lucide icons | https://github.com/lucide-icons/lucide | ISC |
| Geist and Geist Mono | https://github.com/vercel/geist-font | SIL OFL-1.1 |

[Complete installed production JavaScript notices](docs/licenses/javascript.txt) are also included
in the web build as `/THIRD_PARTY_LICENSES.txt`. [Rust dependency notices](docs/licenses/rust.txt)
are included with the container; regenerate with `node scripts/rust_notices.mjs` after lockfile changes.
The GPL text is included in [LICENSE](LICENSE).

The Stockfish installer retains COPYING, AUTHORS and README with release tag, source URL and SHA-256
provenance in each installed version directory. When distributing a binary or container, preserve those
notices and satisfy Stockfish's corresponding-source requirements for the exact release. The image
includes its source/provenance record. Do not substitute a modified engine without updating its source offer.

Archived design studies also contain Inter, Fraunces and IBM Plex fonts under SIL OFL-1.1; their original
copyright and license texts are retained in `docs/design-explorations/licenses`. They are design resources,
separate from the production font bundle. The studies preserve earlier work; the interface follows study A (see README).
