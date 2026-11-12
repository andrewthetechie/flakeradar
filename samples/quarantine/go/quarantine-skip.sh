#!/usr/bin/env bash
# Prints a regular expression for go test's -skip flag that skips every
# top-level test on FlakeRadar's quarantine list, in the form Go expects.
#
# Example input, this sample's quarantine.json:
#
#   [
#     {"suite": "example.com/quarantine-example", "classname": "example.com/quarantine-example",
#      "name": "TestAddsAnItem", "fingerprint": "0000000000000000000000000000000000000001",
#      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
#      "reason": "flaky in CI", "expires_at": "2026-12-01T12:00:00.000000Z"},
#     {"suite": "example.com/quarantine-example", "classname": "example.com/quarantine-example",
#      "name": "TestCheckoutTotal", "fingerprint": "0000000000000000000000000000000000000002",
#      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
#      "reason": null, "expires_at": null}
#   ]
#
# prints exactly:
#
#   ^(TestAddsAnItem|TestCheckoutTotal)$
#
# Usage:
#
#   skip=$(./quarantine-skip.sh); go test ${skip:+-skip "$skip"} ./...
#
# Requires Go 1.20 or later.
#
# Two limits: a subtest, whose name contains /, is left out, because
# go test -skip cannot skip it alone; and names are not scoped to a package,
# so a quarantined name is skipped in every package the run tests.
#
# The environment is FLAKERADAR_URL, FLAKERADAR_TOKEN, FLAKERADAR_REPO (falling
# back to GITHUB_REPOSITORY) and FLAKERADAR_PROJECT; an empty variable counts as
# unset. When the URL or the repo is unset nothing is fetched, and an empty
# line is printed. On any fetch or parse failure one warning line goes to
# stderr and an empty line is printed: this snippet never fails a run and never
# skips a test it could not look up. See ../README.md for the full contract.
#
# Only bash, curl and jq are used (all present on GitHub-hosted runners).
set -euo pipefail

url="${FLAKERADAR_URL:-}"
token="${FLAKERADAR_TOKEN:-}"
repo="${FLAKERADAR_REPO:-${GITHUB_REPOSITORY:-}}"
project="${FLAKERADAR_PROJECT:-}"

# Off: with no server or no repo there is nothing to ask, so nothing is
# skipped: one empty line on stdout, nothing on stderr.
if [[ -z "$url" || -z "$repo" ]]; then
  printf '\n'
  exit 0
fi

# The query is percent-encoded by jq's @uri, as actions/report/report.sh builds
# its ingest query; an empty project is dropped, so the server uses its default.
query=$(jq -rn \
  --arg repo "$repo" --arg project "$project" \
  '[ {k: "repo", v: $repo}, {k: "project", v: $project} ]
   | map(select(.v != ""))
   | map(.k + "=" + (.v | @uri))
   | join("&")')

# The API key is sent only when a token is set.
curl_args=(--fail-with-body -sS --max-time 10)
if [[ -n "$token" ]]; then
  curl_args+=(-H "X-API-Key: ${token}")
fi

# Fail open on a connection error, a timeout or a non-2xx answer. The `if !`
# guard is what keeps `set -e` from ending the script here. curl's own error text
# is dropped, so the warning below is the one line written to stderr.
if ! body=$(curl "${curl_args[@]}" "${url%/}/api/quarantine?${query}" 2>/dev/null); then
  echo "flakeradar-quarantine: quarantine list unreachable; skipping nothing" >&2
  printf '\n'
  exit 0
fi

# A valid body is one JSON value: an array whose every element is an object with
# a string classname and a non-empty string name. The body is slurped first, so a
# body holding several values, such as `{} [{...}]`, is rejected here instead of
# running the filters below once per value and printing one pattern per array.
# Anything else fails open.
if ! items=$(jq -ser '
    select(length == 1)
    | .[0]
    | select(
        type == "array"
        and all(.[];
          type == "object"
          and (.classname | type == "string")
          and (.name | type == "string" and length > 0)))' <<<"$body"); then
  echo "flakeradar-quarantine: invalid quarantine list; skipping nothing" >&2
  printf '\n'
  exit 0
fi

# A subtest's name is Parent/sub; -skip cannot skip it alone, so it is left out
# of the pattern and one warning line names it.
subtests=$(jq -r '.[] | select(.name | contains("/")) | .name' <<<"$items")
if [ -n "$subtests" ]; then
  while IFS= read -r sub; do
    echo "flakeradar-quarantine: ignoring subtest ${sub}; go test -skip cannot skip it alone" >&2
  done <<<"$subtests"
fi

# The top-level names in list order, deduplicated keeping the first, each with
# every regexp metacharacter escaped by a backslash. An empty result prints an
# empty line, which the usage above leaves out of the go test command.
pattern=$(jq -r '
    [ .[] | select((.name | contains("/")) | not) | .name ]
    | reduce .[] as $n ([]; if index($n) == null then . + [$n] else . end)
    | map(gsub("(?<ch>[\\\\.+*?()|\\[\\]{}^$])"; "\\\(.ch)"))
    | if length == 0 then "" else "^(" + join("|") + ")$" end' <<<"$items")
printf '%s\n' "$pattern"
