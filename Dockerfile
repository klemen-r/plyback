# Plyback - container image for server deployments.
# An access password is required: pass PLYBACK_PASSWORD on first start or run `plyback set-password`.
FROM node:22-slim AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

FROM python:3.12-slim
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PLYBACK_DATA=/data \
    PLYBACK_HOST=0.0.0.0 \
    PLYBACK_PORT=8000 \
    PLYBACK_STOCKFISH=/opt/stockfish/stockfish
WORKDIR /app
COPY backend/requirements.txt backend/requirements.txt
RUN pip install --no-cache-dir --require-hashes -r backend/requirements.txt
COPY scripts/install_stockfish.py scripts/install_stockfish.py
RUN python scripts/install_stockfish.py --dest /opt/stockfish
COPY backend/plyback backend/plyback
COPY --from=web /web/dist frontend/dist
COPY LICENSE THIRD_PARTY_NOTICES.md ./
COPY docs/licenses/ docs/licenses/
RUN useradd --system --uid 10001 --home-dir /data plyback && mkdir -p /data && chown plyback /data
USER plyback
VOLUME ["/data"]
EXPOSE 8000
WORKDIR /app/backend
HEALTHCHECK --interval=30s --timeout=5s CMD python -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8000/api/health')"
CMD ["python", "-m", "plyback"]
