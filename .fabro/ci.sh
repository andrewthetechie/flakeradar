#!/usr/bin/env bash
set -euo pipefail
# Validation mirrors .github/workflows/ci.yml: backend ruff, then the suite under
# coverage (backend/.coveragerc's `fail_under`); frontend typecheck, eslint, prettier
# and vitest with coverage on. Exit non-zero on failure.
#
# The Test environment (PostgreSQL preparation and FLAKERADAR_TEST_DATABASE_URL) is declared
# in .fabro/test.toml and read through `fabro-io test-env`. Nothing here repeats it.
test_env=$(fabro-io test-env); eval "$test_env"

cd backend
.venv/bin/ruff check .
.venv/bin/ruff format --check .
# ci.yml, backend job: `coverage run -m pytest` then `coverage report`. A bare
# `pytest` never applies .coveragerc's `fail_under` floor.
.venv/bin/coverage run -m pytest
.venv/bin/coverage report
cd ../frontend
npm run typecheck
npm run lint
npm run format:check
# ci.yml, frontend job. vite.config.ts enables coverage and its thresholds only when
# CI is set, so `npm test` alone measures nothing here. Set CI for this command alone:
# [env] is shared with the backend, which behaves differently under CI.
CI=1 npm test
