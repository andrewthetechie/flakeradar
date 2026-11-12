#!/usr/bin/env bash
# Test for the Go quarantine snippet.
#
# The six shim cases drive quarantine-skip.sh with a `curl` shim: an executable
# file named curl in a temp dir that is put first on PATH, appends its arguments
# one per line to $SHIM_LOG, prints the file $SHIM_BODY, and exits with
# $SHIM_EXIT (default 0). Each case prints `ok <case>` or `not ok <case>`;
# the script exits 1 when any case failed.
#
# The two Go-backed cases run the example module under example/ with the real
# toolchain, and fail when it is missing.
#
# Only bash, curl, jq and coreutils are used, plus (for those two cases) go and
# gotestsum. Run it directly:
#
#   samples/quarantine/go/test.sh
set -uo pipefail

sample_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
snippet="${sample_dir}/quarantine-skip.sh"
fixture="${sample_dir}/quarantine.json"
example_dir="${sample_dir}/example"

work=$(mktemp -d)
# Nothing here writes into the repository: the junit report lands in $work and
# go keeps its build and test caches outside the checkout.
trap 'rm -rf "$work"' EXIT

# The curl shim: it logs its arguments, answers with $SHIM_BODY and exits with
# $SHIM_EXIT. Nothing else calls curl, so what it logs is what the snippet sent.
cat > "${work}/curl" <<'shim'
#!/usr/bin/env bash
printf '%s\n' "$@" >> "$SHIM_LOG"
cat "$SHIM_BODY"
exit "${SHIM_EXIT:-0}"
shim
chmod +x "${work}/curl"
: > "${work}/args"
: > "${work}/body"
export SHIM_LOG="${work}/args"
export SHIM_BODY="${work}/body"
export PATH="${work}:${PATH}"

failures=0

report() {
  if [ "$2" = yes ]; then
    printf 'ok %s\n' "$1"
  else
    printf 'not ok %s\n' "$1"
    failures=$((failures + 1))
  fi
}

# run_snippet <assignments...> runs the snippet with a clean FlakeRadar
# environment and leaves its stdout in ${work}/out, its stderr in ${work}/err,
# and its exit code in $code.
run_snippet() {
  : > "${work}/args"
  : > "${work}/out"
  : > "${work}/err"
  env -u FLAKERADAR_URL -u FLAKERADAR_TOKEN -u FLAKERADAR_REPO \
    -u FLAKERADAR_PROJECT -u GITHUB_REPOSITORY -u SHIM_EXIT \
    "$@" "$snippet" >"${work}/out" 2>"${work}/err"
  code=$?
}

expect_stdout() {
  [ "$(cat "${work}/out")" = "$1" ] && [ "$(wc -l < "${work}/out")" -eq 1 ]
}

# The Go neutral value is an empty line, so an empty stdout check is a check
# for nothing but whitespace.
expect_empty_stdout() {
  [ -z "$(cat "${work}/out")" ]
}

# The snippet never fails the run: every path exits 0.
expect_exit0() {
  if [ "$code" -ne 0 ]; then
    printf 'exit code %s\n' "$code" >&2
    return 1
  fi
}

expect_empty_stderr() {
  [ ! -s "${work}/err" ]
}

# Exactly one stderr line, in the contract's form.
expect_warning() {
  [ "$(grep -Ec '^flakeradar-quarantine: .*; skipping nothing$' "${work}/err")" -eq 1 ]
}

# Exactly this one stderr line.
expect_stderr_line() {
  [ "$(cat "${work}/err")" = "$1" ]
}

expect_shim_url() {
  grep -Fxq "$1" "${work}/args"
}

# --- pattern for the fixture
name='pattern for the fixture'
cp "$fixture" "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app FLAKERADAR_PROJECT=go
expect_stdout '^(TestAddsAnItem|TestCheckoutTotal)$' || ok=no
expect_shim_url 'http://fr/api/quarantine?repo=acme%2Fapp&project=go' || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
report "$name" "$ok"

# --- the two Go-backed cases
# They drive the example module under example/ with the real toolchain. A missing
# go or gotestsum fails them rather than skipping: the sample's Docker image
# installs both, so a skip would hide a broken image.
if ! command -v go >/dev/null 2>&1; then
  toolchain_reason='go is missing'
elif ! command -v gotestsum >/dev/null 2>&1; then
  toolchain_reason='gotestsum is missing'
else
  toolchain_reason=''
fi

# run_go <args...> runs go inside the example module, so its go.mod applies.
run_go() {
  ( cd "$example_dir" && go "$@" )
}

# run_gotestsum <args...> runs gotestsum inside the example module.
run_gotestsum() {
  ( cd "$example_dir" && gotestsum "$@" )
}

# --- fixture names are what gotestsum junit writes
name='fixture names are what gotestsum junit writes'
ok=yes
junit="${work}/junit.xml"
if [ -n "$toolchain_reason" ]; then
  echo "$toolchain_reason" >&2
  ok=no
else
  # A run of the module must pass and write the report.
  if ! run_gotestsum --junitfile "$junit" -- ./... \
      >"${work}/gotestsum.out" 2>"${work}/gotestsum.err"; then
    echo 'gotestsum failed:' >&2
    cat "${work}/gotestsum.out" "${work}/gotestsum.err" >&2
    ok=no
  elif [ ! -f "$junit" ]; then
    echo "gotestsum wrote no ${junit}" >&2
    ok=no
  else
    # For every fixture item, the report must hold that item's testcase element:
    # the classname and name the endpoint answers are exactly what gotestsum
    # wrote, in the order gotestsum writes the two attributes.
    items=$(jq -r '.[] | [.classname, .name] | @tsv' "$fixture")
    if [ -z "$items" ]; then
      echo "$fixture holds no items, so the check would prove nothing" >&2
      ok=no
    else
      while IFS=$'\t' read -r classname test_name; do
        if ! grep -Fq "<testcase classname=\"${classname}\" name=\"${test_name}\"" "$junit"; then
          printf 'no testcase for %s %s in the junit report\n' "$classname" "$test_name" >&2
          ok=no
        fi
      done <<<"$items"
    fi
  fi
fi
report "$name" "$ok"

# --- quarantined tests are not run
name='quarantined tests are not run'
ok=yes
if [ -n "$toolchain_reason" ]; then
  echo "$toolchain_reason" >&2
  ok=no
else
  # The pattern is the snippet's own answer for the fixture rather than a
  # literal copy of it, so this case checks what the snippet really prints.
  cp "$fixture" "${work}/body"
  run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app FLAKERADAR_PROJECT=go
  pattern=$(cat "${work}/out")
  if [ -z "$pattern" ]; then
    echo 'the snippet printed no pattern for the fixture' >&2
    ok=no
  elif ! run_go test -v -skip "$pattern" ./... >"${work}/gotest.out" 2>"${work}/gotest.err"; then
    echo 'go test failed:' >&2
    cat "${work}/gotest.out" "${work}/gotest.err" >&2
    ok=no
  else
    if ! grep -Fq '=== RUN   TestAddsAnItemTwice' "${work}/gotest.out"; then
      echo 'go test -v printed no === RUN   TestAddsAnItemTwice' >&2
      ok=no
    fi
    # A quarantined parent is skipped together with its whole subtree, so the
    # only identifier go test starts is the one test the fixture leaves alone.
    ran=$(sed -n 's/^=== RUN   //p' "${work}/gotest.out")
    if [ "$ran" != 'TestAddsAnItemTwice' ]; then
      echo 'the tests go test started:' >&2
      printf '%s\n' "$ran" >&2
      ok=no
    fi
  fi
fi
report "$name" "$ok"

# --- subtest items are left out
name='subtest items are left out'
cat > "${work}/body" <<'items'
[
  {
    "suite": "example.com/quarantine-example",
    "classname": "example.com/quarantine-example",
    "name": "TestCheckoutTotal/with_discount",
    "fingerprint": "0000000000000000000000000000000000000001",
    "file": null,
    "line": null,
    "quarantined_at": "2026-11-01T12:00:00.000000Z",
    "reason": "flaky in CI",
    "expires_at": "2026-12-01T12:00:00.000000Z"
  },
  {
    "suite": "example.com/quarantine-example",
    "classname": "example.com/quarantine-example",
    "name": "TestAddsAnItem",
    "fingerprint": "0000000000000000000000000000000000000002",
    "file": null,
    "line": null,
    "quarantined_at": "2026-11-01T12:00:00.000000Z",
    "reason": null,
    "expires_at": null
  }
]
items
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_stdout '^(TestAddsAnItem)$' || ok=no
expect_stderr_line 'flakeradar-quarantine: ignoring subtest TestCheckoutTotal/with_discount; go test -skip cannot skip it alone' || ok=no
expect_exit0 || ok=no
report "$name" "$ok"

# --- empty list
name='empty list'
printf '[]\n' > "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_empty_stdout || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
report "$name" "$ok"

# --- no url
name='no url'
ok=yes
run_snippet FLAKERADAR_REPO=acme/app
expect_empty_stdout || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
if [ -s "${work}/args" ]; then
  echo "the shim was called" >&2
  ok=no
fi
report "$name" "$ok"

# --- unreachable
name='unreachable'
cp "$fixture" "${work}/body"
ok=yes
run_snippet SHIM_EXIT=7 FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_empty_stdout || ok=no
expect_exit0 || ok=no
expect_warning || ok=no
report "$name" "$ok"

# --- bad body
name='bad body'
printf '{"detail":"x"}\n' > "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_empty_stdout || ok=no
expect_exit0 || ok=no
expect_warning || ok=no
report "$name" "$ok"

if [ "$failures" -gt 0 ]; then
  exit 1
fi
exit 0
