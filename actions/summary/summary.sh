#!/usr/bin/env bash
# Write which of this CI job's failed tests FlakeRadar already knows as flaky to
# the GitHub job summary. Companion to the reporter step: it reads
# GET /api/ci-runs/{ci_run_id}/failures?repo=...&ci_run_attempt=... until the
# attempt's reports are processed, keeps only this job's failures, writes a
# Markdown table, and fails the job only in gate mode (`gate: true`). See
# ../../docs/ci-integration.md "Failures of one CI run".
#
# Only bash, curl and jq are used (all present on GitHub-hosted runners).
set -uo pipefail

: "${FR_URL:?FR_URL (the FlakeRadar base URL) is required}"
# Action-metadata input defaults cannot use the github/job contexts, so resolve
# the run's own values here from the runner environment. Explicit inputs win.
FR_REPO="${FR_REPO:-${GITHUB_REPOSITORY:-}}"
FR_RUN_ID="${FR_RUN_ID:-${GITHUB_RUN_ID:-}}"
: "${FR_REPO:?FR_REPO (owner/name) is required}"
: "${FR_RUN_ID:?FR_RUN_ID (the CI run id) is required}"
FR_ATTEMPT="${FR_ATTEMPT:-${GITHUB_RUN_ATTEMPT:-1}}"
FR_JOB_ID="${FR_JOB_ID:-}"
FR_TIMEOUT="${FR_TIMEOUT:-60}"
FR_DASHBOARD_URL="${FR_DASHBOARD_URL:-}"
FR_TOKEN="${FR_TOKEN:-}"
FR_SUMMARY_FILE="${FR_SUMMARY_FILE:-}"
FR_POLL_INTERVAL="${FR_POLL_INTERVAL:-2}"
# Gate mode: "true" makes the step fail the job for one of this job's failures
# that is not quarantined. Anything else keeps the read-only behaviour, where
# every path exits 0.
FR_GATE="${FR_GATE:-false}"
# The test step's own outcome (success / failure / cancelled / skipped). Only gate
# mode reads it, and only when FlakeRadar cannot decide the exit itself.
FR_TEST_OUTCOME="${FR_TEST_OUTCOME:-}"
# PR comment mode: "true" keeps one FlakeRadar comment per job on the pull
# request up to date (the send block lives after the poll). Whether a comment
# applies to this run is decided here, from the runner environment: the event
# file names the pull request, and a fork PR never gets one.
FR_PR_COMMENT="${FR_PR_COMMENT:-false}"
FR_GITHUB_TOKEN="${FR_GITHUB_TOKEN:-}"
FR_COMMENT_KEY="${FR_COMMENT_KEY:-}"
comment_key="$FR_COMMENT_KEY"
if [ -z "$comment_key" ]; then
  # Action-metadata input defaults cannot use the github/job contexts, so the
  # workflow and job name stand in for the key here, not in the action metadata.
  comment_key="${GITHUB_WORKFLOW:-}/${GITHUB_JOB:-}"
fi
comment_pr=""
comment_repo=""
if [ "$FR_PR_COMMENT" = "true" ] && [ -n "${GITHUB_EVENT_PATH:-}" ] \
   && [ -f "${GITHUB_EVENT_PATH}" ]; then
  comment_pr=$(jq -r '.pull_request.number // ""' "$GITHUB_EVENT_PATH" 2>/dev/null) \
    || comment_pr=""
  comment_repo=$(jq -r '.pull_request.base.repo.full_name // ""' "$GITHUB_EVENT_PATH" 2>/dev/null) \
    || comment_repo=""
  head_repo=$(jq -r '.pull_request.head.repo.full_name // ""' "$GITHUB_EVENT_PATH" 2>/dev/null) \
    || head_repo=""
  # Not a pull-request event, or a fork (the head repository differs from the
  # base one, or is null, and the token is scoped to the base repository): the
  # comment does not apply to this run, so nothing is sent.
  if [ -z "$comment_pr" ] || [ -z "$comment_repo" ] || [ "$head_repo" != "$comment_repo" ]; then
    comment_pr=""
    comment_repo=""
  fi
fi

# The failures endpoint reports per-attempt counts for the whole run (all jobs).
# We poll until pending reaches 0, then keep only the failures this job owns
# (their ci_job_id names us), because the other jobs write their own summaries.
endpoint="${FR_URL%/}/api/ci-runs/${FR_RUN_ID}/failures"

enc_repo=$(jq -rn --arg r "$FR_REPO" '$r | @uri')

# Write every summary block to the step summary, and to an extra file when one
# was requested (the self-test uses that stable path to inspect the content).
step_file="${GITHUB_STEP_SUMMARY:-}"
extra_file="$FR_SUMMARY_FILE"
if [ -z "$step_file" ] && [ -z "$extra_file" ] && [ "$FR_GATE" != "true" ] \
   && [ "$FR_PR_COMMENT" != "true" ]; then
  # Nothing to write and nothing to decide. With the gate on, or the comment on,
  # the script still polls and decides or sends, and append_summary below
  # tolerates two empty targets.
  echo "::notice::FlakeRadar summary: no summary target (GITHUB_STEP_SUMMARY is unset); nothing to write to."
  exit 0
fi
append_summary() {
  if [ -n "$step_file" ]; then
    printf '%s\n' "$1" >> "$step_file" 2>/dev/null || true
  fi
  if [ -n "$extra_file" ]; then
    printf '%s\n' "$1" >> "$extra_file"
  fi
}

# Fail-closed fallback: when FlakeRadar cannot answer for itself, the gate hands
# the decision to the test step and says so in one notice line. Called with the
# `rule` that could not decide; it sets `gate_exit` to 0 only when the test step
# itself succeeded, and to 1 otherwise. With the gate off it sets 0 and prints
# nothing, which keeps every gate-off path exactly as it was.
gate_exit=0
gate_fallback() {
  gate_exit=0
  if [ "$FR_GATE" != "true" ]; then
    return
  fi
  echo "::notice::FlakeRadar gate: $1; the decision falls back to test-outcome ($FR_TEST_OUTCOME)."
  if [ "$FR_TEST_OUTCOME" != "success" ]; then
    gate_exit=1
  fi
}

# One request: writes the response body to the file named first and prints the
# HTTP status. 000 means curl itself failed (network / DNS / connection refused).
# The summary read always times out at 20s and never retries -- unlike the ingest
# upload in ../report/report.sh, which does -- so this stays script-internal.
# Everything after the file goes straight to curl: method, headers, body, URL.
http_request() {
  local out="$1"
  shift
  local code
  code=$(curl -sS -o "$out" -w '%{http_code}' --max-time 20 "$@" 2>/dev/null) || code="000"
  printf '%s\n' "$code"
}

# --- Poll until this attempt's reports are processed, or the timeout ends. ---
resp_file="$(mktemp)"
trap 'rm -f "$resp_file"' EXIT
deadline=$(( $(date +%s) + FR_TIMEOUT ))
poll_interval="$FR_POLL_INTERVAL"
saw_response=false                        # a 200 that had a pending>0 count
unreachable_line="**FlakeRadar:** could not reach FlakeRadar to fetch this CI run's failures; the job summary was not written."
timeout_line="**FlakeRadar:** timed out after ${FR_TIMEOUT}s waiting for this CI run's reports to finish processing; the job summary was not written."

while :; do
  now=$(date +%s)
  http_code=$(http_request "$resp_file" \
    -H "X-API-Key: ${FR_TOKEN}" \
    "$endpoint?repo=${enc_repo}&ci_run_attempt=${FR_ATTEMPT}")

  if [ "$http_code" = "200" ]; then
    pending=$(jq -r '.reports.pending // "unknown"' "$resp_file" 2>/dev/null)
    if [ "$pending" = "0" ]; then
      break
    fi
    if [ "$pending" != "unknown" ]; then
      saw_response=true
    fi
  fi

  if [ "$now" -ge "$deadline" ]; then
    if [ "$saw_response" = true ]; then
      append_summary "$timeout_line"
      echo "::notice::FlakeRadar summary: reports did not finish processing within ${FR_TIMEOUT}s."
      gate_fallback timeout
    else
      append_summary "$unreachable_line"
      echo "::notice::FlakeRadar summary: the failures endpoint was unreachable."
      gate_fallback unreachable
    fi
    exit "$gate_exit"               # 0 with the gate off, as always
  fi

  sleep "$poll_interval"
done

# The one definition of "this job's failures" and of "known flaky", prepended to
# the table's, the comment rule's and the gate's jq programs so the three cannot
# drift apart: the gate decides on what the table rendered because both read
# `rows`. $job is FR_JOB_ID; an empty $job (a run with no job id) keeps every
# row.
#
# A failure is "known flaky" when FlakeRadar already knows the test: it passed
# on retry in this run, it is a flaky test, it is broken on the default branch,
# or it has proven flakes (it passed and failed on one commit SHA). Everything
# else is a new failure the change may have caused. This mirrors the "Proven
# flakes" column, so the headline count and the table never disagree.
JQ_ROWS='def this_job: ($job == "" or .ci_job_id == $job);
         def rows: [ .failures[] | select(this_job) ];
         def known_flaky: (.status == "passed") or .tier == "flaky" or .broken == true or .confirmed_flake_count > 0;'

# --- Build the Markdown table for this job's failures. ---
markdown=$(jq -r \
  --arg job "$FR_JOB_ID" \
  --arg dash "$FR_DASHBOARD_URL" \
  --arg repo "$FR_REPO" "$JQ_ROWS"'
  def esc: gsub("\\|"; "\\|");
  rows | . as $fs
    | ( $fs | length ) as $total
    | ( [ $fs[] | select(known_flaky) ] | length ) as $known
    | if $total == 0 then
        "**FlakeRadar:** no failed tests reported for this CI job."
      else
        ("**FlakeRadar:** \($known) of \($total) failures are known flaky tests.\n\n" +
         "| Test | Status | Tier (score) | Proven flakes | Quarantined | Issue |\n" +
         "|---|---|---|---|---|---|\n" +
         ( [ $fs[] | (
             (if $dash != "" then "[\(.name | esc)](\($dash)?repo=\($repo|@uri)&test=\(.test_id))"
              else .name | esc end) as $test |
             (.status | if . == "passed" then "passed on retry" else . end) as $st |
             ((.tier) + " (" + ((.flakiness_score * 100) | floor / 100 | tostring) + ")") as $td |
             ((.confirmed_flake_count | tostring)) as $pf |
             (if .quarantined then "yes" else "no" end) as $q |
             (if (.github_issue_url != null and .github_issue_url != "")
                then "[#\(.github_issue_number)](\(.github_issue_url))" else "—" end) as $iss |
             "| \($test) | \($st | esc) | \($td) | \($pf) | \($q) | \($iss) |"
           ) ] | join("\n") ))
      end
' "$resp_file")

append_summary "$markdown"

# --- Keep one FlakeRadar comment per job on the pull request up to date. ---
# The comment's first line is the marker that finds it again on a re-run; the
# rest is the same Markdown the job summary got above. Every failed GitHub call
# prints one warning and never changes the exit: this block runs before the
# gate, so a job the gate is about to fail still gets its comment, and with the
# gate off every path here stays exit 0.
if [ -n "$comment_pr" ] && [ -n "$comment_repo" ]; then
  marker="<!-- flakeradar-summary:${comment_key} -->"
  comment_body="${marker}
${markdown}"
  api_url="${GITHUB_API_URL:-https://api.github.com}"
  gh_headers=(
    -H "Authorization: Bearer ${FR_GITHUB_TOKEN}"
    -H "Accept: application/vnd.github+json"
    -H "X-GitHub-Api-Version: 2022-11-28"
  )
  comment_file="$(mktemp)"
  trap 'rm -f "$resp_file" "$comment_file"' EXIT

  # Find our earlier comment: list page by page until a page holds fewer than
  # 100 comments, and take the first comment whose body starts with the marker.
  comment_id=""
  list_ok=true                  # a failed list leaves the state unknown
  page=1
  while :; do
    http_code=$(http_request "$comment_file" "${gh_headers[@]}" \
      "${api_url}/repos/${comment_repo}/issues/${comment_pr}/comments?per_page=100&page=${page}")
    if [ "$http_code" != "200" ]; then
      echo "::warning::FlakeRadar summary: listing the pull request's comments failed (HTTP ${http_code})."
      list_ok=false
      break
    fi
    found=$(jq -r --arg m "$marker" \
      '[.[] | select(.body | startswith($m))][0].id // ""' "$comment_file" 2>/dev/null) || found=""
    if [ -n "$found" ]; then
      # The first match on the first page that holds one wins: a later page's
      # duplicate (from an earlier bug or a copied comment) is never edited, so
      # the comment the reader sees first is the one kept up to date.
      comment_id="$found"
      break
    fi
    # A short page was the last one; a full one may have a successor.
    count=$(jq 'length' "$comment_file" 2>/dev/null) || count=0
    if [ "$count" -lt 100 ]; then
      break
    fi
    page=$((page + 1))
  done

  if [ -n "$comment_id" ]; then
    # Our comment is already there: edit it, also when this job now has no
    # failures, so a green re-run clears the stale table in place.
    comment_json=$(jq -cn --arg body "$comment_body" '{body: $body}')
    http_code=$(http_request "$comment_file" -X PATCH "${gh_headers[@]}" \
      -H "Content-Type: application/json" \
      -d "$comment_json" \
      "${api_url}/repos/${comment_repo}/issues/comments/${comment_id}")
    case "$http_code" in
      2*) ;;
      *) echo "::warning::FlakeRadar summary: updating the pull request comment failed (HTTP ${http_code})." ;;
    esac
  elif [ "$list_ok" = true ] && [ "$(jq -r --arg job "$FR_JOB_ID" "$JQ_ROWS"'
      (rows | length) > 0
    ' "$resp_file" 2>/dev/null)" = "true" ]; then
    # Nothing of ours exists, and the job has failures (`rows`, the table's own
    # selection): post the first comment. A green job without a comment stays
    # silent instead of announcing its emptiness. A failed listing above leaves
    # the state unknown — our comment may exist — so nothing is posted: a
    # warning was already printed, and skipping beats duplicating the comment
    # on the next re-run.
    comment_json=$(jq -cn --arg body "$comment_body" '{body: $body}')
    http_code=$(http_request "$comment_file" -X POST "${gh_headers[@]}" \
      -H "Content-Type: application/json" \
      -d "$comment_json" \
      "${api_url}/repos/${comment_repo}/issues/${comment_pr}/comments")
    case "$http_code" in
      2*) ;;
      *) echo "::warning::FlakeRadar summary: creating the pull request comment failed (HTTP ${http_code})." ;;
    esac
  fi
fi

# --- Gate mode: fail the job for a failure that is not quarantined. ---
# The gate reads `rows`, the very selection the table rendered and the comment
# rule tests, so it decides on what the reader just saw. A failure whose newest
# status is "passed" passed on retry and is therefore not a failure of this job.
# resp_file still holds the polled answer.
if [ "$FR_GATE" = "true" ]; then
  # An answer that cannot settle the question falls back too: a list capped at
  # `truncated`, or a Report that failed to process and so lists nothing. A failed
  # read is an incomplete answer as well, so only a clear "no" keeps deciding.
  incomplete=$(jq -r 'if (.truncated == true or ((.reports.failed // 0) > 0))
                       then "yes" else "no" end' "$resp_file" 2>/dev/null)
  if [ "$incomplete" != "no" ]; then
    gate_fallback 'the failures list is incomplete'
    exit "$gate_exit"
  fi
  gate_blockers=$(jq -r --arg job "$FR_JOB_ID" "$JQ_ROWS"'
    [ rows[]
      | select(.status != "passed" and .quarantined == false)
      | .name ] | join(", ")' "$resp_file" 2>/dev/null)
  if [ -n "$gate_blockers" ] && [ "$gate_blockers" != "null" ]; then
    echo "::error::FlakeRadar gate: failures that are not quarantined: $gate_blockers"
    exit 1
  fi
fi
exit 0
