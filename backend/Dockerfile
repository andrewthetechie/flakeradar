FROM node:22-slim AS frontend
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM python:3.12-slim
# Mirror the repo layout: main.py resolves ../../frontend/dist from app/.
WORKDIR /srv/backend
COPY backend/requirements.txt .
RUN pip install --no-cache-dir --require-hashes -r requirements.txt
COPY backend/alembic.ini ./
COPY backend/migrations ./migrations
COPY backend/app ./app
COPY --from=frontend /build/dist /srv/frontend/dist
# Everything the app writes lives in PostgreSQL (migrations run in the lifespan),
# so /srv stays root-owned and read-only: keep Python from trying to drop
# __pycache__ next to the sources, then drop root privileges.
ENV PYTHONDONTWRITEBYTECODE=1
RUN useradd --system --uid 10001 --create-home flakeradar
USER flakeradar
EXPOSE 8000
# No HTTP client binary in the slim image, so the probe uses urllib: an HTTP
# error or connection failure raises and the check exits non-zero. The start
# period covers the migrations that run in the lifespan.
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8000/api/health', timeout=2)"
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", "--workers", "2"]
