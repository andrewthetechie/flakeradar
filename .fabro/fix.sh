#!/usr/bin/env bash
# Formatters and machine-applicable lint fixes ONLY (fabro-workflows ADR 0011 D5).
# ci.sh is the real gate, so every step runs even if an earlier one fails and this
# script always exits 0.
set -u

(cd backend && .venv/bin/ruff check --fix . && .venv/bin/ruff format .) || true
(cd frontend && npm run lint:fix && npm run format) || true

exit 0
