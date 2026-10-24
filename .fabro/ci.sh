#!/usr/bin/env bash
set -euo pipefail
# Validation mirrors .github/workflows/ci.yml: backend ruff + pytest, frontend typecheck,
# eslint, prettier and vitest. Exit non-zero on failure.
#
# The Test environment (PostgreSQL preparation and FLAKERADAR_TEST_DATABASE_URL) is declared
# in .fabro/test.toml and read through `fabro-io test-env`. Nothing here repeats it.
test_env=$(fabro-io test-env); eval "$test_env"

cd backend
.venv/bin/ruff check .
.venv/bin/ruff format --check .
.venv/bin/pytest
cd ../frontend
npm run typecheck
npm run lint
npm run format:check
npm test
