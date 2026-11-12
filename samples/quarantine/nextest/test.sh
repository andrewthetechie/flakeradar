#!/usr/bin/env bash
# Test for the cargo-nextest quarantine snippet.
#
# The five cases here drive quarantine-filter.sh with a `curl` shim: an
# executable file named curl in a temp dir that is put first on PATH, appends
# its arguments one per line to $SHIM_LOG, prints the file $SHIM_BODY, and
# exits with $SHIM_EXIT (default 0). Each case prints `ok <case>` or
# `not ok <case>`; the script exits 1 when any case failed.
#
# The two cargo-backed cases of README.md's *Test expectations* arrive with the
# example crate and need a Rust toolchain.
#
# Only bash, curl, jq and coreutils are used. Run it directly:
#
#   samples/quarantine/nextest/test.sh
set -uo pipefail

sample_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
snippet="${sample_dir}/quarantine-filter.sh"
fixture="${sample_dir}/quarantine.json"

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

if [ "$failures" -gt 0 ]; then
  exit 1
fi
exit 0
