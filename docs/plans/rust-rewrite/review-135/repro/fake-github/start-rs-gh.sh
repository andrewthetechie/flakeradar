#!/bin/bash
. <S>/run/env.sh
export FLAKERADAR_DATABASE_URL=postgresql://fr:pw@127.0.0.1:55917/rs FLAKERADAR_PORT=18082 FLAKERADAR_GITHUB_TOKEN=ghtok GH_BASE=http://127.0.0.1:18092
cd <S>/run
exec <S>/target/debug/flakeradar-patched
