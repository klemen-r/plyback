#!/usr/bin/env bash
# Plyback - start the server.   ./start.sh [--lan] [--port 8000]
set -euo pipefail
cd "$(dirname "$0")"
[ -x .venv/bin/python ] || { echo "Run ./setup.sh first."; exit 1; }
export PLYBACK_HOST=127.0.0.1 PLYBACK_PORT=8000
NO_BROWSER=0
while [ $# -gt 0 ]; do
  case "$1" in
    --lan) PLYBACK_HOST=0.0.0.0 ;;
    --port) [ $# -ge 2 ] || { echo "--port requires a number"; exit 1; }; PLYBACK_PORT="$2"; shift ;;
    --no-browser) NO_BROWSER=1 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
  shift
done
echo "Plyback is starting at http://localhost:$PLYBACK_PORT  (Ctrl+C to stop)"
[ "$NO_BROWSER" = 1 ] || ( sleep 3; (command -v xdg-open >/dev/null && xdg-open "http://localhost:$PLYBACK_PORT") || (command -v open >/dev/null && open "http://localhost:$PLYBACK_PORT") ) >/dev/null 2>&1 &
cd backend && exec ../.venv/bin/python -m plyback
