#!/usr/bin/env bash
# Test for the cargo-nextest quarantine snippet.
#
# The seven cases here drive quarantine-filter.sh with a `curl` shim or a real
# toolchain: an executable file named curl in a temp dir that is put first on PATH,
# appends its arguments one per line to $SHIM_LOG, prints the file $SHIM_BODY, and
# exits with $SHIM_EXIT (default 0). Each case prints `ok <case>` or
# `not ok <case>`; the script exits 1 when any case failed.
#
# The two cargo-backed cases run the example crate under example/ and need a
# Rust toolchain with cargo-nextest; they fail when it is missing.
#
# Only bash, curl, jq, coreutils and (for those two cases) cargo and
# cargo-nextest are used. Run it directly:
#
#   samples/quarantine/nextest/test.sh
set -uo pipefail

sample_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
snippet="${sample_dir}/quarantine-filter.sh"
fixture="${sample_dir}/quarantine.json"
example_dir="${sample_dir}/example"

work=$(mktemp -d)
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

expect_shim_url() {
  grep -Fxq "$1" "${work}/args"
}

# --- expression for the fixture
name='expression for the fixture'
cp "$fixture" "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app FLAKERADAR_PROJECT=rust
expect_stdout 'not ((binary_id(=quarantine-example::cart) & test(=adds_an_item)) | (binary_id(=quarantine-example::cart) & test(=checkout_total)))' || ok=no
expect_shim_url 'http://fr/api/quarantine?repo=acme%2Fapp&project=rust' || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
report "$name" "$ok"

# --- empty list
name='empty list'
printf '[]\n' > "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_stdout 'all()' || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
report "$name" "$ok"

# --- no url
name='no url'
ok=yes
run_snippet FLAKERADAR_REPO=acme/app
expect_stdout 'all()' || ok=no
expect_empty_stderr || ok=no
expect_exit0 || ok=no
if [ -s "${work}/args" ]; then
  echo "the shim was called" >&2
  ok=no
fi
report "$name" "$ok"

# --- no repo
name='no repo'
cp "$fixture" "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr
expect_stdout 'all()' || ok=no
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
expect_stdout 'all()' || ok=no
expect_exit0 || ok=no
expect_warning || ok=no
report "$name" "$ok"

# --- bad body
name='bad body'
printf '{"detail":"x"}\n' > "${work}/body"
ok=yes
run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
expect_stdout 'all()' || ok=no
expect_exit0 || ok=no
expect_warning || ok=no
report "$name" "$ok"

# --- bad body with several JSON values
# The endpoint answers one array, so a body that concatenates values is invalid
# even when one of them is a valid list. jq without -s reads such a body value by
# value and takes its -e exit code from the last one, so an unguarded script
# skips the invalid-body branch and prints one expression per array: `{} [...]`
# prints that expression alone, and `[] [...]` prints `all()` plus the expression.
name='bad body with several JSON values'
ok=yes
for multi_body in '{} [{"classname":"x","name":"y"}]' '[] [{"classname":"x","name":"y"}]'; do
  printf '%s\n' "$multi_body" > "${work}/body"
  run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app
  if ! expect_stdout 'all()'; then
    printf 'stdout for [%s]:\n%s\n' "$multi_body" "$(cat "${work}/out")" >&2
    ok=no
  fi
  expect_exit0 || ok=no
  if ! expect_warning; then
    printf 'stderr for [%s]:\n%s\n' "$multi_body" "$(cat "${work}/err")" >&2
    ok=no
  fi
done
report "$name" "$ok"

# --- the two cargo-backed cases
# They drive the example crate under example/ with the real toolchain. A missing
# cargo or cargo-nextest fails them rather than skipping: the sample's Docker
# image installs both, so a skip would hide a broken image.
if ! command -v cargo >/dev/null 2>&1; then
  toolchain_reason='cargo is missing'
elif ! cargo nextest --version >/dev/null 2>&1; then
  toolchain_reason='cargo-nextest is missing'
else
  toolchain_reason=''
fi

# run_cargo <args...> runs cargo nextest inside the example crate, so its own
# .config/nextest.toml applies.
run_cargo() {
  ( cd "$example_dir" && cargo nextest "$@" )
}

# The cargo cases create example/Cargo.lock and example/target/. Neither is
# committed, so this script removes whatever it created and leaves anything that
# was already there.
lock_existed=no
target_existed=no
[ -e "${example_dir}/Cargo.lock" ] && lock_existed=yes
[ -e "${example_dir}/target" ] && target_existed=yes
clean_example() {
  if [ "$target_existed" = no ]; then
    rm -rf "${example_dir}/target"
  fi
  if [ "$lock_existed" = no ]; then
    rm -f "${example_dir}/Cargo.lock"
  fi
}
trap 'rm -rf "$work"; clean_example' EXIT

# --- fixture names are what nextest junit writes
name='fixture names are what nextest junit writes'
ok=yes
junit="${example_dir}/target/nextest/default/junit.xml"
if [ -n "$toolchain_reason" ]; then
  echo "$toolchain_reason" >&2
  ok=no
else
  # A run of the crate must pass, and its default profile must write the report.
  if ! run_cargo run >"${work}/run.out" 2>"${work}/run.err"; then
    echo 'cargo nextest run failed:' >&2
    cat "${work}/run.err" >&2
    ok=no
  elif [ ! -f "$junit" ]; then
    echo "cargo nextest run wrote no ${junit}" >&2
    ok=no
  else
    # For every fixture item, the report must hold that item's testcase element:
    # the classname and name the endpoint answers are exactly what nextest wrote.
    items=$(jq -r '.[] | [.classname, .name] | @tsv' "$fixture")
    if [ -z "$items" ]; then
      echo "$fixture holds no items, so the check would prove nothing" >&2
      ok=no
    else
      while IFS=$'\t' read -r classname test_name; do
        if ! grep -Fq "<testcase name=\"${test_name}\" classname=\"${classname}\"" "$junit"; then
          printf 'no testcase for %s %s in %s\n' "$classname" "$test_name" "$junit" >&2
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
  # The expression is the snippet's own answer for the fixture rather than a
  # literal copy of it, so this case checks what the snippet really prints.
  cp "$fixture" "${work}/body"
  run_snippet FLAKERADAR_URL=http://fr FLAKERADAR_REPO=acme/app FLAKERADAR_PROJECT=rust
  expression=$(cat "${work}/out")
  if ! run_cargo list -E "$expression" >"${work}/list.out" 2>"${work}/list.err"; then
    echo 'cargo nextest list failed:' >&2
    cat "${work}/list.err" >&2
    ok=no
  elif [ "$(cat "${work}/list.out")" != 'quarantine-example::cart adds_an_item_twice' ]; then
    echo 'the filtered list holds:' >&2
    cat "${work}/list.out" >&2
    ok=no
  fi
fi
report "$name" "$ok"

clean_example

if [ "$failures" -gt 0 ]; then
  exit 1
fi
exit 0
