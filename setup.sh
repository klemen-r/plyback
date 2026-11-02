#!/usr/bin/env bash
# Plyback - one-time setup for macOS / Linux.
set -euo pipefail
cd "$(dirname "$0")"
PY=""
for c in python3 python; do
  if command -v "$c" >/dev/null && "$c" -c 'import sys; sys.exit(0 if sys.version_info >= (3,10) else 1)'; then PY="$c"; break; fi
done
[ -z "$PY" ] && { echo "Python 3.10+ is required (https://www.python.org/downloads/)"; exit 1; }
[ -d .venv ] || "$PY" -m venv .venv
.venv/bin/python -m pip install --upgrade pip --quiet
.venv/bin/python -m pip install --require-hashes -r backend/requirements.txt --quiet
echo "==> Python dependencies installed"
.venv/bin/python scripts/install_stockfish.py
if command -v npm >/dev/null; then
  node -e "const [m,n]=process.versions.node.split('.').map(Number); process.exit(m<22||(m===22&&n<12)?1:0)" || { echo "Node.js 22.12+ is required"; exit 1; }
  (cd frontend && npm ci --no-audit --no-fund && npm run build)
  echo "==> Web interface built"
elif [ -f frontend/dist/index.html ]; then
  echo "==> Node.js not found; using the pre-built web interface"
else
  echo "Node.js 22.12+ is needed to build the web interface (https://nodejs.org)"; exit 1
fi
echo
echo "Setup complete. Start the app with:  ./start.sh"
