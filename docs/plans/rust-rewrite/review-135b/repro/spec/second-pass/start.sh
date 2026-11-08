#!/bin/bash
S=/tmp/sp2
export FLAKERADAR_API_TOKEN=tok-spec-review FLAKERADAR_WORKER_POLL_SECONDS=0.05
(cd /tmp/python-base/backend && FLAKERADAR_DATABASE_URL=postgresql+asyncpg://fr:pw@127.0.0.1:55971/py nohup /tmp/pyref/bin/uvicorn app.main:app --host 127.0.0.1 --port 18181 --workers 1 > $S/py.log 2>&1 &)
(cd $S && FLAKERADAR_DATABASE_URL=postgresql://fr:pw@127.0.0.1:55971/rs FLAKERADAR_PORT=18182 nohup /Users/andrew/Documents/code/flakeradar/target/debug/flakeradar > $S/rs.log 2>&1 &)
