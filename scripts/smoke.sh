#!/usr/bin/env bash
# Smoke-test a built FlakeRadar binary the way a deployment runs it.
#
# Usage: scripts/smoke.sh <binary> [database-url]
#
# The database URL defaults to $FLAKERADAR_DATABASE_URL. The script starts the
# binary from an empty temporary directory on a port of its own, waits for the
# database to be ready, uploads one JUnit Report, waits for the processor and
# then checks every surface the binary carries: the read API, the dashboard (when
# the build embedded one), the API docs, the MCP tools, and the healthcheck
# subcommand. It ends by proving the shipped default token is refused.
#
# Bash, curl and the usual POSIX tools only, so it runs on Linux, macOS and Git
# Bash on Windows. No jq: the runners do not all carry it, and the bodies it
# reads are small.
set -euo pipefail

TOKEN="smoke-token"
PROTOCOL_VERSION="2026-07-28"
MCP_TOOLS=11

fail() {
  printf 'smoke: FAIL: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'smoke: %s\n' "$*"
}

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  fail "usage: scripts/smoke.sh <binary> [database-url]"
fi

binary="$1"
database_url="${2:-${FLAKERADAR_DATABASE_URL:-}}"
[ -n "$database_url" ] || fail "no database URL: pass one or set FLAKERADAR_DATABASE_URL"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
fixture="$repo_root/actions/report/test/fixture-alpha.xml"
[ -f "$fixture" ] || fail "the JUnit fixture is missing at $fixture"

case "$binary" in
  /* | [A-Za-z]:[\\/]*) ;;
  *) binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")" ;;
esac
[ -x "$binary" ] || fail "not an executable: $binary"

workdir="$(mktemp -d)"
log="$workdir/flakeradar.log"
pid=""

cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    # A process that ignores the signal must not keep the runner busy.
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.5
    done
    kill -KILL "$pid" 2>/dev/null || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT

# 1. Run the binary from an empty directory, so no .env and no stray file is read.
port=$((20000 + (RANDOM % 20000)))
base_url="http://127.0.0.1:$port"
note "starting $binary on $base_url"
# Started from the temporary directory, in this shell, so `wait` sees it as a
# child and its exit status is the one the shutdown check reads.
pushd "$workdir" >/dev/null
FLAKERADAR_DATABASE_URL="$database_url" \
  FLAKERADAR_API_TOKEN="$TOKEN" \
  FLAKERADAR_HOST=127.0.0.1 \
  FLAKERADAR_PORT="$port" \
  "$binary" >"$log" 2>&1 &
pid=$!
popd >/dev/null

died() {
  printf 'smoke: FAIL: %s\n' "$*" >&2
  printf 'smoke: the binary log follows\n' >&2
  tail -n 40 "$log" >&2 || true
  exit 1
}

# A helper that answers the body of `url`, or fails the run.
fetch() {
  curl --fail-with-body -sS --max-time 10 "$@" || died "request failed: $*"
}

# 2. Wait for the database, not just the process: /api/health/ready says whether
#    the pool answers and the processor holds its lock.
ready=""
for _ in $(seq 1 60); do
  kill -0 "$pid" 2>/dev/null || died "the binary exited before it was ready"
  ready="$(curl -sS --max-time 2 "$base_url/api/health/ready" || true)"
  case "$ready" in
    *'"database":true'* | *'"database": true'*) break ;;
  esac
  ready=""
  sleep 0.5
done
[ -n "$ready" ] || died "/api/health/ready never reported database:true (last answer: $ready)"
note "ready: $ready"

# 3. Upload one JUnit Report, the way actions/report/report.sh does.
upload="$(curl --fail-with-body -sS --max-time 30 -X POST \
  "$base_url/api/ingest?repo=smoke/alpha&project=default&commit_sha=smoke-sha&branch=main&default_branch=main&ci_run_id=smoke-run-1&ci_job_id=smoke-job" \
  -H "X-API-Key: $TOKEN" \
  -H "Content-Type: application/xml" \
  --data-binary "@$fixture")" || died "POST /api/ingest failed"
report_id="$(printf '%s' "$upload" | sed -n 's/.*"report_id"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')"
case "$upload" in
  *'"status":"pending"'* | *'"status": "pending"'*) ;;
  *) died "ingest did not queue a pending Report: $upload" ;;
esac
[ -n "$report_id" ] || died "ingest answered no Report id: $upload"
note "queued Report $report_id"

# 4. Wait for the processor to turn it into a Run.
status=""
for _ in $(seq 1 60); do
  kill -0 "$pid" 2>/dev/null || died "the binary exited while a Report was pending"
  body="$(curl -sS --max-time 5 "$base_url/api/reports/$report_id" || true)"
  status="$(printf '%s' "$body" | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([a-z]*\)".*/\1/p')"
  case "$status" in
    processed) break ;;
    failed) died "Report $report_id failed: $body" ;;
  esac
  status=""
  sleep 0.5
done
[ "$status" = "processed" ] || died "Report $report_id was never processed"
note "Report $report_id processed"

# 5. The Tests the Report carried are listed.
# `include_stable` keeps a Test whose score is still zero in the list, which a
# single Report produces.
tests="$(fetch "$base_url/api/tests?repo=smoke/alpha&include_stable=true")"
for name in test_cart_total_sums_line_items test_refund_requires_an_order; do
  case "$tests" in
    *"$name"*) ;;
    *) died "/api/tests does not list $name: $tests" ;;
  esac
done
note "/api/tests lists both Tests"

# 6. The dashboard, when this build embedded one. An API-only build answers the
#    API's 404 here, and the script says so rather than pretending it checked.
index_code="$(curl -sS --max-time 5 -o "$workdir/index.html" -w '%{http_code}' "$base_url/")"
case "$index_code" in
  200)
    grep -q '<div id="root"' "$workdir/index.html" \
      || died "/ did not serve the dashboard's root element"
    asset="$(grep -o '/assets/[A-Za-z0-9._-]*' "$workdir/index.html" | head -n 1)"
    [ -n "$asset" ] || died "/ served an index.html that names no asset"
    asset_code="$(curl -sS --max-time 10 -o "$workdir/asset" -w '%{http_code}' "$base_url$asset")"
    [ "$asset_code" = "200" ] || died "$asset answered $asset_code"
    [ -s "$workdir/asset" ] || died "$asset served an empty body"
    note "the dashboard and $asset are served"
    ;;
  404)
    note "no dashboard is embedded in this build; / answers 404"
    ;;
  *) died "/ answered $index_code" ;;
esac

# 7. The API docs and the document they render.
docs_code="$(curl -sS --max-time 10 -o "$workdir/docs.html" -w '%{http_code}' "$base_url/docs")"
[ "$docs_code" = "200" ] || died "/docs answered $docs_code"
openapi_code="$(curl -sS --max-time 10 -o "$workdir/openapi.json" -w '%{http_code}' "$base_url/openapi.json")"
[ "$openapi_code" = "200" ] || died "/openapi.json answered $openapi_code"
grep -q '"openapi"' "$workdir/openapi.json" || died "/openapi.json is not an OpenAPI document"
note "/docs and /openapi.json are served"

# 8. MCP answers tools/list over its bearer token.
cat >"$workdir/tools-list.json" <<EOF
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"$PROTOCOL_VERSION","io.modelcontextprotocol/clientCapabilities":{}}}}
EOF
tools="$(curl --fail-with-body -sS --max-time 15 -X POST "$base_url/mcp/" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "MCP-Protocol-Version: $PROTOCOL_VERSION" \
  -H "Mcp-Method: tools/list" \
  --data-binary "@$workdir/tools-list.json")" || died "POST /mcp/ tools/list failed"
tool_count="$(printf '%s' "$tools" | grep -o '"inputSchema"' | wc -l | tr -d '[:space:]')" || true
[ "$tool_count" = "$MCP_TOOLS" ] || died "tools/list carried $tool_count tools, expected $MCP_TOOLS"
note "MCP tools/list carries $MCP_TOOLS tools"

# 9. The healthcheck subcommand, from the same empty directory, exits 0.
(
  cd "$workdir"
  FLAKERADAR_DATABASE_URL="$database_url" FLAKERADAR_PORT="$port" "$binary" healthcheck \
    >"$workdir/healthcheck.log" 2>&1
) || died "flakeradar healthcheck failed: $(cat "$workdir/healthcheck.log")"
note "flakeradar healthcheck exited 0"

# 10. Stop the process and confirm it shut down cleanly.
kill -TERM "$pid"
set +e
wait "$pid"
exit_code=$?
set -e
pid=""
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    # A native Windows process cannot receive SIGTERM, so Git Bash terminates it
    # and the exit status says nothing about the shutdown path. The in-process
    # shutdown test in crates/flakeradar/tests/server.rs covers it instead.
    note "stopped on Windows (exit $exit_code); a graceful SIGTERM exit is not provable here"
    ;;
  *)
    [ "$exit_code" = "0" ] || died "the binary exited $exit_code on SIGTERM, not 0"
    note "the binary shut down cleanly"
    ;;
esac

# Separately: the shipped default token, with no opt-in, must be refused before
# anything is served.
pushd "$workdir" >/dev/null
default_code=0
FLAKERADAR_DATABASE_URL="$database_url" FLAKERADAR_PORT="$port" "$binary" \
  >"$workdir/default-token.log" 2>&1 || default_code=$?
popd >/dev/null
[ "$default_code" -ne 0 ] || died "the binary served with the default token"
note "the default token is refused (exit $default_code)"

note "OK"
