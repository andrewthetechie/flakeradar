#!/usr/bin/env bash
set -euo pipefail
# Idempotent dependency install: backend venv (Python via uv) and the frontend's npm packages.
# CI pins ruff in .github/workflows/ci.yml; keep this in step with it.
cd backend
uv venv --python 3.13 --allow-existing .venv
uv pip install --python .venv/bin/python -r requirements.txt ruff==0.16.9
cd ../frontend
npm ci
