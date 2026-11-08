#!/usr/bin/env bash
# Upload one or more JUnit XML files to FlakeRadar: each matched file is POSTed
# as its own Report to /api/ingest, with this run's metadata computed from the
# runner environment. The accepted Report ids are written as the `report_ids`
# action output, so a later step (or the summary action) can follow them. When no
# file matches, a warning is written and nothing is uploaded. See
# ../../docs/ci-integration.md.
#
# Only bash, curl and jq are used (all present on GitHub-hosted runners).
set -uo pipefail

: "${FR_URL:?FR_URL (the FlakeRadar base URL) is required}"
: "${FR_TOKEN:?FR_TOKEN (the FlakeRadar API token) is required}"

# Action-metadata input defaults cannot use the github/job contexts, so resolve
# the run's own values here from the runner environment. Explicit env wins.
FR_REPO="${FR_REPO:-${GITHUB_REPOSITORY:-}}"
FR_SHA="${FR_SHA:-${GITHUB_SHA:-}}"
FR_BRANCH="${FR_BRANCH:-${GITHUB_REF_NAME:-}}"
FR_RUN_ID="${FR_RUN_ID:-${GITHUB_RUN_ID:-}}"
FR_ATTEMPT="${FR_ATTEMPT:-${GITHUB_RUN_ATTEMPT:-1}}"
# ci_job_id and default_branch are never user inputs: the composite step passes
# them from the job/event contexts (see action.yml).
FR_JOB_ID="${FR_JOB_ID:-}"
: "${FR_REPO:?FR_REPO (owner/name) is required}"
: "${FR_SHA:?FR_SHA (the commit SHA) is required}"

FR_DEFAULT_BRANCH="${FR_DEFAULT_BRANCH:-}"
if [ -z "$FR_DEFAULT_BRANCH" ] && [ -n "${GITHUB_EVENT_PATH:-}" ] \
   && [ -f "${GITHUB_EVENT_PATH}" ]; then
  FR_DEFAULT_BRANCH=$(jq -r '.repository.default_branch // ""' "$GITHUB_EVENT_PATH" 2>/dev/null) \
    || FR_DEFAULT_BRANCH=""
fi

# Derive the pipeline (workflow file path, e.g. .github/workflows/ci.yml) from
# GITHUB_WORKFLOW_REF = owner/repo/.github/workflows/ci.yml@refs/heads/main,
# exactly as the CI snippet does.
PIPELINE="${GITHUB_WORKFLOW_REF:-}"
PIPELINE="${PIPELINE#${GITHUB_REPOSITORY:-}/}"
PIPELINE="${PIPELINE%@*}"

FR_PROJECT="${FR_PROJECT:-default}"
FR_ROOT="${FR_ROOT:-}"
FR_LINE_BASE="${FR_LINE_BASE:-}"
FR_FILES="${FR_FILES:-junit.xml}"
FR_FAIL_ON_ERROR="${FR_FAIL_ON_ERROR:-false}"
fail_on_error=false
case "$FR_FAIL_ON_ERROR" in
  true|TRUE|True|1|yes) fail_on_error=true ;;
esac

# --- Match the files to upload: each match becomes one Report. ---
# FR_FILES is newline-separated glob patterns, taken relative to the workspace.
base="${GITHUB_WORKSPACE:-.}"
matched=$'\n'
count=0

match_glob() {                       # $1: one glob pattern
  local pat="$1" full f
  case "$pat" in
    /*) full="$pat" ;;
    *) full="${base%/}/$pat" ;;
  esac
  # Split expanded paths on newlines only, so a workspace path with a space
  # survives; nullglob makes a non-matching pattern expand to nothing.
  local IFS=$'\n'
  for f in $full; do
    [ -f "$f" ] || continue          # skip directories and unreadable entries
    case "$matched" in
      *$'\n'"$f"$'\n'*) ;;           # already matched by an earlier pattern
      *)
        matched="$matched$f"$'\n'
        count=$((count + 1))
        ;;
    esac
  done
}

shopt -s nullglob
while IFS= read -r pattern; do
  # Tolerate the indentation and blank lines a block scalar brings with it.
  pattern="${pattern#"${pattern%%[![:space:]]*}"}"
  pattern="${pattern%"${pattern##*[![:space:]]}"}"
  [ -n "$pattern" ] || continue
  match_glob "$pattern"
done <<< "$FR_FILES"
shopt -u nullglob

write_output() {                     # $1: comma-separated Report ids
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'report_ids=%s\n' "$1" >> "$GITHUB_OUTPUT" \
      || echo "::warning::FlakeRadar report: could not write the report_ids output."
  else
    echo "::notice::FlakeRadar report: report_ids=$1 (GITHUB_OUTPUT is unset, so nothing was recorded as an output)."
  fi
}

if [ "$count" -eq 0 ]; then
  echo "::warning::FlakeRadar report: no files matched the pattern(s): ${FR_FILES//$'\n'/ }. Nothing was uploaded."
  write_output ""
  exit 0                             # no match is not an upload failure
fi

# --- Upload each matched file as its own Report. ---
resp_file="$(mktemp)"
trap 'rm -f "$resp_file"' EXIT
ids=""
last_code="000"
last_detail=""

upload_file() {                      # $1: the file to POST
  local file="$1" query
  # Build and URL-encode the query string so values with / or spaces stay
  # intact; optional parameters that are empty are left out entirely.
  query=$(jq -rn \
    --arg repo "$FR_REPO" --arg sha "$FR_SHA" --arg branch "$FR_BRANCH" \
    --arg project "$FR_PROJECT" --arg run "$FR_RUN_ID" --arg attempt "$FR_ATTEMPT" \
    --arg job "$FR_JOB_ID" --arg pipeline "$PIPELINE" \
    --arg default_branch "$FR_DEFAULT_BRANCH" --arg root "$FR_ROOT" \
    --arg line_base "$FR_LINE_BASE" \
    '[ {k: "repo", v: $repo}, {k: "commit_sha", v: $sha}, {k: "branch", v: $branch},
       {k: "project", v: $project}, {k: "ci_run_id", v: $run},
       {k: "ci_run_attempt", v: $attempt}, {k: "ci_job_id", v: $job},
       {k: "pipeline", v: $pipeline}, {k: "default_branch", v: $default_branch},
       {k: "root", v: $root}, {k: "line_base", v: $line_base} ]
     | map(select(.v != ""))
     | map(.k + "=" + (.v | @uri))
     | join("&")')
  # 000 means curl itself failed (network / DNS / connection refused).
  last_code=$(curl --fail-with-body -sS -o "$resp_file" -w '%{http_code}' \
    --retry 5 --retry-all-errors --retry-delay 2 -X POST \
    "${FR_URL%/}/api/ingest?${query}" \
    -H "X-API-Key: ${FR_TOKEN}" \
    -H "Content-Type: application/xml" \
    --data-binary "@${file}" 2>/dev/null)
  [ -n "$last_code" ] || last_code="000"
  last_detail=$(jq -r '.detail // .error // ""' "$resp_file" 2>/dev/null)
  # Keep the server message on one line and short: it goes into a workflow
  # command, where a newline could forge another command.
  last_detail="${last_detail//$'\n'/ }"
  last_detail="${last_detail//$'\r'/}"
  last_detail="${last_detail:0:200}"
  local report_id=""
  case "$last_code" in
    200|201|202) report_id=$(jq -r '.report_id // empty' "$resp_file" 2>/dev/null) ;;
  esac
  if [ -z "$report_id" ]; then
    return 1
  fi
  ids="${ids:+${ids},}${report_id}"
  echo "::notice::FlakeRadar report: uploaded ${file} as Report ${report_id}."
  return 0
}

failed=0
while IFS= read -r file; do
  [ -n "$file" ] || continue
  if upload_file "$file"; then
    continue
  fi
  failed=$((failed + 1))
  # Every file is attempted even when fail-on-error is true: the step still
  # fails at the end, and the uploads that did work are worth reporting.
  if [ "$fail_on_error" = true ]; then
    echo "::error::FlakeRadar report: ${file} was not accepted (HTTP ${last_code}${last_detail:+: ${last_detail}})."
  else
    echo "::warning::FlakeRadar report: ${file} was not accepted (HTTP ${last_code}${last_detail:+: ${last_detail}})."
  fi
done <<< "$matched"

write_output "$ids"

if [ "$failed" -gt 0 ]; then
  if [ "$fail_on_error" = true ]; then
    echo "::error::FlakeRadar report: ${failed} of ${count} file(s) failed to upload."
    exit 1
  fi
  echo "::warning::FlakeRadar report: ${failed} of ${count} file(s) failed to upload; not failing the build (fail-on-error is false)."
fi
exit 0
