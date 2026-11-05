# Plyback: native Rust server + Stockfish + static interface. No Python runtime.
FROM node:22-slim AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build
COPY scripts/install_stockfish.mjs /tmp/install_stockfish.mjs
# CI supplies an optional BuildKit secret; it is never stored in image layers.
RUN --mount=type=secret,id=github_token \
    GITHUB_TOKEN="$(cat /run/secrets/github_token 2>/dev/null || true)" \
    node /tmp/install_stockfish.mjs --dest /opt/stockfish

FROM rust:1.97-bookworm AS native
WORKDIR /src
COPY rust/Cargo.toml rust/Cargo.lock rust/
COPY rust/openapi.json rust/openapi.json
COPY rust/src rust/src
COPY rust/migrations rust/migrations
COPY backend/plyback/data/openings.json backend/plyback/data/openings.json
RUN cargo build --release --locked --manifest-path rust/Cargo.toml

FROM debian:bookworm-slim
ENV PLYBACK_DATA=/data \
    PLYBACK_FRONTEND=/app/frontend/dist \
    PLYBACK_HOST=0.0.0.0 \
    PLYBACK_PORT=8000 \
    PLYBACK_STOCKFISH=/opt/stockfish/stockfish
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libstdc++6 libgomp1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=native /src/rust/target/release/plyback /usr/local/bin/plyback
COPY --from=web /opt/stockfish /opt/stockfish
COPY --from=web /web/dist frontend/dist
COPY LICENSE THIRD_PARTY_NOTICES.md ./
COPY docs/licenses/ docs/licenses/
RUN useradd --system --uid 10001 --home-dir /data plyback && mkdir -p /data && chown plyback /data
USER plyback
VOLUME ["/data"]
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s CMD ["plyback", "healthcheck"]
CMD ["plyback"]
