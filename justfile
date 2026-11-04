# Optional macOS / Linux shortcuts. Run `just` to see the commands.
set shell := ["bash", "-euo", "pipefail", "-c"]
set positional-arguments

# Show available commands.
default:
    @just --list

# Install locked runtime dependencies, Stockfish and the built interface.
setup:
    bash setup.sh

# Prepare runtime, development tools and the browser used by tests.
setup-dev: setup dev-deps browser-install

# Install the locked Python development tools (after setup).
dev-deps:
    python3 -m venv .venv
    .venv/bin/python -m pip install --require-hashes -r backend/requirements-dev.txt

# Run locally; optional flags: just run --port 8080 --no-browser.
run *args:
    bash start.sh "$@"

# Run the frontend with live updates; keep `just run` in another terminal.
dev:
    cd frontend && npm run dev

# Build the production native server and interface.
build:
    cargo build --release --locked --manifest-path rust/Cargo.toml
    cd frontend && npm run build

# Check native, preserved baseline and frontend code style.
lint:
    cargo fmt --manifest-path rust/Cargo.toml --check
    cargo clippy --locked --manifest-path rust/Cargo.toml --all-targets -- -D warnings
    cd backend && ../.venv/bin/python -m ruff check --config pyproject.toml plyback tests bench ../scripts
    cd frontend && npm run lint

# Run backend and frontend unit tests with isolated data; no live Chess.com requests.
test:
    node --test scripts/install_stockfish.test.mjs
    cargo test --release --locked --manifest-path rust/Cargo.toml
    cd backend && ../.venv/bin/python -m pytest -m "not live"
    cd frontend && npm test

# Install Chromium for browser tests.
browser-install:
    cd frontend && npx --no-install playwright install chromium

# Run browser workflows with separate fixture servers and test data.
e2e:
    cd frontend && npm run e2e

# Run all local checks, including a fresh build before browser tests.
check: lint test build e2e

# Install or verify the local Stockfish engine.
stockfish:
    node scripts/install_stockfish.mjs

# Set or change the access password interactively.
password:
    rust/target/release/plyback set-password

# Save a consistent backup to a NEW path (relative paths start at the repo root).
backup destination:
    rust/target/release/plyback backup "$1"

# Check the running local server; optionally: just status 8080.
status port="8000":
    curl --fail --silent --show-error --max-time 5 "http://127.0.0.1:$1/api/health"
    @printf '\n'

# Follow the application's rotating log (Ctrl+C stops following).
logs:
    tail -n 50 -f "${PLYBACK_DATA:-data}/logs/plyback.log"

# Check locked dependencies for known vulnerabilities.
audit:
    cargo audit --file rust/Cargo.lock --deny warnings
    .venv/bin/python -m pip_audit --disable-pip --no-deps -r backend/requirements-dev.txt
    cd frontend && npm audit

# Refresh supported versions, install them, then run every local check. Requires uv.
update-deps:
    cargo update --manifest-path rust/Cargo.toml
    node scripts/rust_notices.mjs
    uv pip compile backend/pyproject.toml --python-version 3.11 --universal --generate-hashes --upgrade --output-file backend/requirements.txt --quiet
    uv pip compile backend/pyproject.toml --python-version 3.11 --extra dev --universal --generate-hashes --upgrade --constraint backend/requirements.txt --output-file backend/requirements-dev.txt --quiet
    .venv/bin/python -m pip install --require-hashes -r backend/requirements-dev.txt
    cd frontend && npm update --save --no-audit --no-fund
    just check
