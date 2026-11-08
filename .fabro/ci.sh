#!/usr/bin/env bash
set -euo pipefail
# Validation mirrors .github/workflows/ci.yml: the release version check, rustfmt,
# clippy with warnings as errors, the nextest suite and cargo audit, then the
# frontend's typecheck, eslint, prettier and vitest with coverage on. Exit
# non-zero on failure.
#
# The Test environment (PostgreSQL preparation and FLAKERADAR_TEST_DATABASE_URL)
# is declared in .fabro/test.toml and read through `fabro-io test-env`. Nothing
# here repeats it.
test_env=$(fabro-io test-env); eval "$test_env"

# version-consistency job: the workspace version and the frontend must agree.
node scripts/version.mjs check

# `rust` job: the workflow sets RUSTFLAGS="-D warnings" globally, and clippy is
# also given -D warnings explicitly because it reads that from its own argument.
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features --profile ci
cargo test --doc --all-features
cargo audit

# `frontend` job.
cd frontend
npm run typecheck
npm run lint
npm run format:check
# vite.config.ts enables coverage and its thresholds only when CI is set, so
# `npm test` alone measures nothing here. Set CI for this command alone: [env] in
# test.toml is shared with the Rust suite, which behaves differently under CI.
CI=1 npm test
