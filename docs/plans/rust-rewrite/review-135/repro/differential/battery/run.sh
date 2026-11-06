#!/usr/bin/env bash
# Run the differential sweep: one PostgreSQL container, one database per server,
# the reference and this port started side by side on identical settings, then
# the battery from `main.py`.
#
#   ./run.sh                 # the default profile: D1, D2, D3, D4, D5
#   ./run.sh retention       # the retention profile: D6 (a prune that keeps nothing)
#   ./run.sh all             # both profiles, one after the other
#
# The reference is a checkout of commit 86c98804's `backend/` and a virtualenv
# with its requirements; both are outside the repo (see ../README.md).
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(git -C "$HERE" rev-parse --show-toplevel)
PY_BASE=${PY_BASE:-/tmp/python-base}
PY_VENV=${PY_VENV:-/tmp/pyref}
PG_CONTAINER=${PG_CONTAINER:-t23-pg}
PG_PORT=${PG_PORT:-55917}
PY_PORT=${PY_PORT:-18081}
RS_PORT=${RS_PORT:-18082}
WEBHOOK_PY_PORT=${WEBHOOK_PY_PORT:-18291}
WEBHOOK_RS_PORT=${WEBHOOK_RS_PORT:-18292}
PUBLIC_URL=https://flake.example.com
OUT=${OUT:-$HERE/out}
TOKEN=tok-spec-review
PROFILE=${1:-default}

mkdir -p "$OUT"

stop_servers() {
  pkill -f "uvicorn app.main:app --host 127.0.0.1 --port $PY_PORT" 2>/dev/null
  pkill -f "target/debug/flakeradar" 2>/dev/null
  pkill -f "webhook_receiver.py" 2>/dev/null
  sleep 0.5
}

start_postgres() {
  docker rm -f "$PG_CONTAINER" >/dev/null 2>&1
  docker run -d --name "$PG_CONTAINER" \
    -e POSTGRES_USER=fr -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=base \
    -p "$PG_PORT:5432" postgres:17-alpine >/dev/null || return 1
  for _ in $(seq 1 60); do
    docker exec "$PG_CONTAINER" pg_isready -U fr -d base >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  echo "postgres never became ready" >&2
  return 1
}

fresh_databases() {
  docker exec "$PG_CONTAINER" psql -U fr -d base -q \
    -c "drop database if exists py with (force)" \
    -c "drop database if exists rs with (force)" \
    -c "create database py" \
    -c "create database rs"
}

start_servers() {
  local extra=("$@")
  (
    cd "$PY_BASE/backend" || exit 1
    env FLAKERADAR_API_TOKEN=$TOKEN \
        FLAKERADAR_WORKER_POLL_SECONDS=0.05 \
        FLAKERADAR_PUBLIC_URL=$PUBLIC_URL \
        FLAKERADAR_WEBHOOK_URLS="http://127.0.0.1:$WEBHOOK_PY_PORT/hook" \
        FLAKERADAR_DATABASE_URL="postgresql+asyncpg://fr:pw@127.0.0.1:$PG_PORT/py" \
        "${extra[@]}" \
        "$PY_VENV/bin/uvicorn" app.main:app --host 127.0.0.1 --port "$PY_PORT" --workers 1 \
        > "$OUT/py.log" 2>&1 &
  )
  (
    cd "$REPO" || exit 1
    env FLAKERADAR_API_TOKEN=$TOKEN \
        FLAKERADAR_WORKER_POLL_SECONDS=0.05 \
        FLAKERADAR_PUBLIC_URL=$PUBLIC_URL \
        FLAKERADAR_WEBHOOK_URLS="http://127.0.0.1:$WEBHOOK_RS_PORT/hook" \
        FLAKERADAR_DATABASE_URL="postgresql://fr:pw@127.0.0.1:$PG_PORT/rs" \
        FLAKERADAR_HOST=127.0.0.1 FLAKERADAR_PORT=$RS_PORT \
        "${extra[@]}" \
        target/debug/flakeradar \
        > "$OUT/rs.log" 2>&1 &
  )
  for _ in $(seq 1 120); do
    a=$(curl -s -m1 -H "X-API-Key: $TOKEN" "http://127.0.0.1:$PY_PORT/api/health/ready")
    b=$(curl -s -m1 "http://127.0.0.1:$RS_PORT/api/health/ready")
    if [ -n "$a" ] && [ -n "$b" ]; then
      echo "reference: $a"
      echo "port:      $b"
      return 0
    fi
    sleep 0.5
  done
  echo "a server never became ready; see $OUT/py.log and $OUT/rs.log" >&2
  return 1
}

run_profile() {
  local profile=$1
  shift
  case "$profile" in
    default)
      phases="seed,d2,d1,d3,d5,d4,d3r"
      env_extra=()
      ;;
    retention)
      phases="seed,d6"
      # Every window stays at its default, so a fixture that is seconds old is
      # untouched; `d6` then ages rows past each window with one statement pair.
      # The failed-Report window is off by default and is on here so the phase
      # can cross it, and the interval is short so a prune round is never far off.
      env_extra=(
        FLAKERADAR_FAILED_REPORT_RETENTION_DAYS=1
        FLAKERADAR_PRUNE_INTERVAL_SECONDS=0.5
      )
      ;;
    *)
      echo "unknown profile $profile" >&2
      return 2
      ;;
  esac

  stop_servers
  fresh_databases || return 1
  rm -f "$OUT/webhooks-py.jsonl" "$OUT/webhooks-rs.jsonl"
  "$PY_VENV/bin/python" "$HERE/webhook_receiver.py" \
    --py-port "$WEBHOOK_PY_PORT" --rs-port "$WEBHOOK_RS_PORT" --out "$OUT" \
    > "$OUT/webhook.log" 2>&1 &
  sleep 0.5
  start_servers "${env_extra[@]}" || return 1

  BATTERY_PG=$PG_CONTAINER BATTERY_PY="http://127.0.0.1:$PY_PORT" BATTERY_RS="http://127.0.0.1:$RS_PORT" \
    PY_BASE=$PY_BASE PY_VENV=$PY_VENV REPO=$REPO \
    PY_BACKEND="$PY_BASE/backend" RS_BIN="$REPO/target/debug/flakeradar" \
    PY_PORT=$PY_PORT RS_PORT=$RS_PORT PG_PORT=$PG_PORT BATTERY_PG_PORT=$PG_PORT BATTERY_OUT=$OUT \
    "$PY_VENV/bin/python" "$HERE/main.py" --profile "$profile" --phases "$phases" \
      --pg "$PG_CONTAINER" --py "http://127.0.0.1:$PY_PORT" --rs "http://127.0.0.1:$RS_PORT" \
      --out "$OUT"
  local status=$?
  echo "report: $OUT/report-$profile.md"
  return $status
}

overall=0
case "$PROFILE" in
  all)
    start_postgres || exit 1
    run_profile default || overall=1
    run_profile retention || overall=1
    ;;
  default|retention)
    start_postgres || exit 1
    run_profile "$PROFILE" || overall=1
    ;;
  *)
    echo "usage: $0 [default|retention|all]" >&2
    exit 2
    ;;
esac
stop_servers
docker rm -f "$PG_CONTAINER" >/dev/null 2>&1
exit $overall
