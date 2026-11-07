#!/bin/bash
. <S>/run/env.sh
export FLAKERADAR_DATABASE_URL=postgresql+asyncpg://fr:pw@127.0.0.1:55917/py
cd <S>/python-base/backend
exec <S>/venv/bin/uvicorn app.main:app --host 127.0.0.1 --port 18081 --workers 1
