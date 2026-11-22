#!/usr/bin/env bash
# Build the native server and interface, and install Stockfish.
set -euo pipefail
cd "$(dirname "$0")"
command -v cargo >/dev/null || { echo "Install Rust from https://rustup.rs and run setup again."; exit 1; }
command -v node >/dev/null || { echo "Node.js 22.12+ is required to build the interface."; exit 1; }
node -e "const [m,n]=process.versions.node.split('.').map(Number); process.exit(m<22||(m===22&&n<12)?1:0)" || { echo "Node.js 22.12+ is required"; exit 1; }
(cd frontend && npm ci --no-audit --no-fund && npm run build)
cargo build --release --locked --manifest-path rust/Cargo.toml
# Last: a failed engine download must not leave the app without its interface.
if [ -z "${PLYBACK_STOCKFISH:-}" ]; then
  node scripts/install_stockfish.mjs
fi
echo "Setup complete. Start with: bash start.sh"
