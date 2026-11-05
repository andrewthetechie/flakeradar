#!/bin/bash
# usage: reset-gh.sh VAR=val ...
pkill -f "uvicorn app.main:app"; pkill -f "uvicorn gh_main:app"; pkill -f "target/debug/flakeradar"; pkill -f fake_gh.py; sleep 1
docker exec pr135-spec-pg psql -U fr -d base -qc "drop database if exists py with (force)" -c "drop database if exists rs with (force)" -c "create database py" -c "create database rs"
cd <S>/run; rm -f gh-py.jsonl gh-rs.jsonl
nohup <S>/venv/bin/python fake_gh.py 18091 gh-py.jsonl >/dev/null 2>&1 &
nohup <S>/venv/bin/python fake_gh.py 18092 gh-rs.jsonl >/dev/null 2>&1 &
nohup env "$@" ./start-py-gh.sh > py.log 2>&1 &
nohup env "$@" ./start-rs-gh.sh > rs.log 2>&1 &
sleep 6
curl -s -m3 localhost:18081/api/health/ready; echo; curl -s -m3 localhost:18082/api/health/ready; echo
