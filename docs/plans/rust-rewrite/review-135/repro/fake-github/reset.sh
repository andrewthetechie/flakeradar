#!/bin/bash
pkill -f "uvicorn app.main:app" ; pkill -f "target/debug/flakeradar"; sleep 1
docker exec pr135-spec-pg psql -U fr -d base -qc "drop database if exists py with (force)" -c "drop database if exists rs with (force)" -c "create database py" -c "create database rs"
cd <S>/run
nohup env $@ ./start-py.sh > py.log 2>&1 &
nohup env $@ ./start-rs.sh > rs.log 2>&1 &
sleep 6
curl -s -m3 localhost:18081/api/health/ready; echo; curl -s -m3 localhost:18082/api/health/ready; echo
