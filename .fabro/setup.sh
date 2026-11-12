#!/usr/bin/env bash
set -euo pipefail
# Idempotent dependency install: the frontend's npm packages, the workspace's
# Cargo dependencies, and cargo-audit if the image does not carry it. ci.sh runs
# with --network=none, so everything it invokes is fetched here.
#
# CI pins cargo-audit in .github/workflows/ci.yml (`CARGO_AUDIT_VERSION`); keep
# the two in step.
(cd frontend && npm ci)
cargo fetch
if ! command -v cargo-audit >/dev/null 2>&1; then
  cargo install cargo-audit --version 0.22.2 --locked
fi

# A PostgreSQL server for the Rust suite. The image's own `fabro-pg-ensure` is
# the normal provider, and a server already installed here is enough for
# .fabro/ensure-postgres.sh to start; when neither exists, install one. Only this
# script has a network, so it is the only place that can.
if ! command -v fabro-pg-ensure >/dev/null 2>&1 && ! command -v pg_lsclusters >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    # Best effort: a box that cannot install a server still fails where it
    # matters, in ci.sh's Test environment, rather than here in front of stages
    # that run no Rust test.
    apt-get update -qq \
      && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends postgresql \
      || echo 'setup: PostgreSQL is not installed and no server is reachable; the Rust suite will fail' >&2
  else
    echo 'setup: no apt-get and no PostgreSQL: the Rust suite will fail' >&2
  fi
fi
