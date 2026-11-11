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

# The failures endpoint reports per-attempt counts for the whole run (all jobs).
# We poll until pending reaches 0, then keep only the failures this job owns
# (their ci_job_id names us), because the other jobs write their own summaries.
endpoint="${FR_URL%/}/api/ci-runs/${FR_RUN_ID}/failures"

enc_repo=$(jq -rn --arg r "$FR_REPO" '$r | @uri')

# Write every summary block to the step summary, and to an extra file when one
# was requested (the self-test uses that stable path to inspect the content).
step_file="${GITHUB_STEP_SUMMARY:-}"
extra_file="$FR_SUMMARY_FILE"
if [ -z "$step_file" ] && [ -z "$extra_file" ] && [ "$FR_GATE" != "true" ]; then
  # Nothing to write and nothing to decide. With the gate on the script still polls
  # and decides, and append_summary below tolerates two empty targets.
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
  # 000 means curl itself failed (network / DNS / connection refused).
  http_code=$(curl -sS -o "$resp_file" -w '%{http_code}' --max-time 20 \
    -H "X-API-Key: ${FR_TOKEN}" \
    "$endpoint?repo=${enc_repo}&ci_run_attempt=${FR_ATTEMPT}" 2>/dev/null) || http_code="000"

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
    else
      append_summary "$unreachable_line"
      echo "::notice::FlakeRadar summary: the failures endpoint was unreachable."
    fi
    exit 0                                # never fail the job
  fi

  sleep "$poll_interval"
done

# --- Build the Markdown table for this job's failures. ---
# A failure is "known flaky" when FlakeRadar already knows the test: it passed
# on retry in this run, it is a flaky test, it is broken on the default branch,
# or it has proven flakes (it passed and failed on one commit SHA). Everything
# else is a new failure the change may have caused. This mirrors the "Proven
# flakes" column, so the headline count and the table never disagree.
markdown=$(jq -r \
  --arg job "$FR_JOB_ID" \
  --arg dash "$FR_DASHBOARD_URL" \
  --arg repo "$FR_REPO" '
  def esc: gsub("\\|"; "\\|");
  .failures
    | map(select($job == "" or .ci_job_id == $job))
    | . as $fs
    | ( $fs | length ) as $total
    | ( [ $fs[] | select((.status == "passed") or .tier == "flaky" or .broken == true or .confirmed_flake_count > 0) ] | length ) as $known
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

# --- Gate mode: fail the job for a failure that is not quarantined. ---
# The same job filter as the table above, so the gate decides on what the reader
# just saw. A failure whose newest status is "passed" passed on retry and is
# therefore not a failure of this job. resp_file still holds the polled answer.
if [ "$FR_GATE" = "true" ]; then
  gate_blockers=$(jq -r --arg job "$FR_JOB_ID" '
    [ .failures[]
      | select($job == "" or .ci_job_id == $job)
      | select(.status != "passed" and .quarantined == false)
      | .name ] | join(", ")' "$resp_file" 2>/dev/null)
  if [ -n "$gate_blockers" ] && [ "$gate_blockers" != "null" ]; then
    echo "::error::FlakeRadar gate: failures that are not quarantined: $gate_blockers"
    exit 1
  fi
fi
exit 0
