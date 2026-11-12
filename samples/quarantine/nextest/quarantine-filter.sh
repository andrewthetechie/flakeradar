#!/usr/bin/env bash
# Prints a cargo-nextest filterset expression that excludes every test on
# FlakeRadar's quarantine list, in the form nextest expects on its -E flag.
#
# Example input, this sample's quarantine.json:
#
#   [
#     {"suite": "quarantine-example::cart", "classname": "quarantine-example::cart",
#      "name": "adds_an_item", "fingerprint": "0000000000000000000000000000000000000001",
#      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
#      "reason": "flaky in CI", "expires_at": "2026-12-01T12:00:00.000000Z"},
#     {"suite": "quarantine-example::cart", "classname": "quarantine-example::cart",
#      "name": "checkout_total", "fingerprint": "0000000000000000000000000000000000000002",
#      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
#      "reason": null, "expires_at": null}
#   ]
#
# prints exactly:
#
#   not ((binary_id(=quarantine-example::cart) & test(=adds_an_item)) | (binary_id(=quarantine-example::cart) & test(=checkout_total)))
#
# Usage:
#
#   cargo nextest run -E "$(./quarantine-filter.sh)"
#
# Several -E flags are ORed, so a team with its own filter combines it with
# this one through `&` instead of passing a second -E:
#
#   cargo nextest run -E "(<own filter>) & $(./quarantine-filter.sh)"
#
# The environment is FLAKERADAR_URL, FLAKERADAR_TOKEN, FLAKERADAR_REPO (falling
# back to GITHUB_REPOSITORY) and FLAKERADAR_PROJECT; an empty variable counts as
# unset. When the URL or the repo is unset nothing is fetched, and the neutral
# expression all() is printed. On any fetch or parse failure one warning line
# goes to stderr and all() is printed: this snippet never fails a run and never
# skips a test it could not look up. See ../README.md for the full contract.
#
# Only bash, curl and jq are used (all present on GitHub-hosted runners).
set -euo pipefail

url="${FLAKERADAR_URL:-}"
token="${FLAKERADAR_TOKEN:-}"
repo="${FLAKERADAR_REPO:-${GITHUB_REPOSITORY:-}}"
project="${FLAKERADAR_PROJECT:-}"

# Off: with no server or no repo there is nothing to ask, so nothing is skipped
# and nothing is written to stderr.
if [[ -z "$url" || -z "$repo" ]]; then
  printf 'all()\n'
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
  printf 'all()\n'
  exit 0
fi

# A valid body is a JSON array whose every element is an object with a string
# classname and a non-empty string name; anything else fails open.
if ! jq -e '
    type == "array"
    and all(.[];
      type == "object"
      and (.classname | type == "string")
      and (.name | type == "string" and length > 0))' >/dev/null 2>&1 <<<"$body"; then
  echo "flakeradar-quarantine: invalid quarantine list; skipping nothing" >&2
  printf 'all()\n'
  exit 0
fi

# One exclusion per item, in list order. An item with no classname excludes by
# test name alone. The names are not escaped: a Rust test name cannot hold `)`
# or a space, so it needs no quoting inside the filterset grammar.
if ! expression=$(jq -r '
    if length == 0 then "all()" else
      map(
        if .classname == "" then "test(=" + .name + ")"
        else "(binary_id(=" + .classname + ") & test(=" + .name + "))"
        end
      )
      | "not (" + (join(" | ")) + ")"
    end' <<<"$body"); then
  echo "flakeradar-quarantine: invalid quarantine list; skipping nothing" >&2
  printf 'all()\n'
  exit 0
fi

printf '%s\n' "$expression"
