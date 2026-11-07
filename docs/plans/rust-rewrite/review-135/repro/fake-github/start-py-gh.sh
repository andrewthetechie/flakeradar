#!/bin/bash
. <S>/run/env.sh
export FLAKERADAR_DATABASE_URL=postgresql+asyncpg://fr:pw@127.0.0.1:55917/py FLAKERADAR_GITHUB_TOKEN=ghtok GH_BASE=http://127.0.0.1:18091
cd <S>/python-base/backend
export PYTHONPATH=<S>/run:.
exec <S>/venv/bin/uvicorn gh_main:app --host 127.0.0.1 --port 18081 --workers 1
