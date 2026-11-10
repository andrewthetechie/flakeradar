#!/usr/bin/env bash
# Build and run every quarantine-consumer sample under samples/quarantine/: one
# Docker image per sample dir, built from that dir's Dockerfile with
# samples/quarantine/ as the build context (so a sample can copy shared files),
# then run with its default command, which runs the sample's own test. Each
# sample is attempted even after one fails; one PASS or FAIL line per sample is
# printed at the end, and the script exits 1 when any sample failed. See
# README.md for the contract every sample follows.
#
# Usage: samples/quarantine/test.sh [sample...]
# Without arguments every sample dir is run, in sorted order.
#
# Only bash and docker are needed.
set -euo pipefail

# Work from the repository root, whatever the caller's working directory is, so
# every path below is repository-relative.
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

samples_dir=samples/quarantine

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

# Collect the sample names to run: the named dirs from the arguments, or every
# direct child dir of samples/quarantine/ that holds a Dockerfile. The glob
# expands in sorted order.
names=()
if [ "$#" -gt 0 ]; then
  # Every name is checked before anything is built, so a typo builds nothing.
  for name in "$@"; do
    if [ ! -f "${samples_dir}/${name}/Dockerfile" ]; then
      echo "unknown quarantine sample: ${name} (no ${samples_dir}/${name}/Dockerfile)" >&2
      exit 2
    fi
  done
  names=("$@")
else
  for dir in "${samples_dir}"/*/; do
    # A glob that matches nothing stays literal, so skip that.
    [ -d "$dir" ] || continue
    if [ -f "${dir}Dockerfile" ]; then
      names+=("$(basename "$dir")")
    fi
  done
fi

if [ "${#names[@]}" -eq 0 ]; then
  echo "no quarantine samples found"
  exit 0
fi

failed=0
results=()
for name in "${names[@]}"; do
  image="flakeradar-quarantine-${name}"
  passed=false
  # Docker's own output passes through. A failed build counts as FAIL and its
  # run is skipped; each build and run is guarded, so a failure does not end
  # the script.
  if docker build -f "${samples_dir}/${name}/Dockerfile" -t "$image" "$samples_dir"; then
    if docker run --rm "$image"; then
      passed=true
    fi
  fi
  if [ "$passed" = true ]; then
    results+=("PASS ${name}")
  else
    results+=("FAIL ${name}")
    failed=$((failed + 1))
  fi
done

# One line per sample, after every sample has run.
for line in "${results[@]}"; do
  printf '%s\n' "$line"
done

if [ "$failed" -gt 0 ]; then
  exit 1
fi
exit 0
