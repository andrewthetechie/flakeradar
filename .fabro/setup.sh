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
