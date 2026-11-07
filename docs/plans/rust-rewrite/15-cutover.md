# 15 — Cutover and the PR

Read `00-shared-context.md` first. Tasks 01–14 have landed.

## Goal

Delete Python, make the docs tell the truth, and open the one PR. Do not merge it, and do not create tags. The user does both.

## Deliverables

### Delete

- `backend/` entirely: `app`, `tests`, `migrations`, requirement files, `Dockerfile`, `alembic.ini`, `.coveragerc`, `pytest.ini`.
- `tools/parity-record/` and `ruff.toml`.
- `scripts/check-test-manifest.mjs` and its CI step. Keep `test-manifest.csv` under `docs/plans/rust-rewrite/` as a record.
- From `ci.yml`: the `backend` job and `feat/rust-rewrite` in `push.branches`.

Keep `samples/simulate_ci.py` and `samples/gitlab/report_pipeline.py`. They are samples, not part of the server. Keep `tests/parity/`, including the golden files, as regression tests. Update `tests/parity/README.md` to say the golden files were recorded from Python at `97b933d` and that the recorder is gone: a golden file changes only with an intended behaviour change and an entry in `differences.md`.

Then search for leftovers across the repo for `uvicorn`, `alembic`, `pytest`, `ruff`, `pip`, `requirements`, `FastAPI`, `SQLAlchemy`, `asyncpg`, `venv` and `backend/`. Each remaining hit must be correct on purpose, such as a historical doc under `docs/plans/` or an ADR.

### Coverage floors

Set `coverage-floor.json` to the measured line and branch totals, rounded down. They must be at least 95 and 80. This PR has no `rust-coverage` baseline, so only the floors apply to it. The first push to `main` creates the baseline.

Update `CONTRIBUTING.md` with the ratchet rule: PRs fail on any drop against `main`, and a PR that adds tests raises the floors in the same change.

### `.fabro/`

Switch to Rust, following writers-app's files. These run after merge, once the new image exists.

- `setup.sh`: `(cd frontend && npm ci)`, `cargo fetch`, and `cargo-audit` if it is missing. It must work with `--network=none` from a warmed image.
- `ci.sh`: mirrors CI: `node scripts/version.mjs check`, fmt, clippy with `-D warnings`, `cargo nextest run --all-features --profile ci`, `cargo audit`, then the frontend's typecheck, lint, format check and `CI=1 npm test`.
- `test.toml`: keep `[env]` and `[prepare]` with `FLAKERADAR_TEST_DATABASE_URL = "postgresql://test:test@localhost:5432/test"`. Add a `rust` target with `run = "cargo nextest run --all-features --profile ci"` and `watch = ["crates/**", "Cargo.toml", "Cargo.lock"]`. Keep the `frontend` target. Replace the `ruff` check with `fmt` and `clippy`.

### Docs

- **`README.md`:**
  - Replace the Python badge.
  - Quick start with a downloaded binary and a database, then Docker.
  - A table of the five targets, stating that Windows ARM is not supported.
  - How to verify checksums and attestations.
  - The macOS Gatekeeper and Windows SmartScreen prompts for unsigned binaries, and how to get past them.
  - New settings: `FLAKERADAR_HOST` and `FLAKERADAR_PORT`, and the `127.0.0.1` default.
  - `/docs` is Redoc (read-only).
  - Remove every Python and uvicorn instruction.
  - **Upgrading from 1.x:** it needs a fresh database, and the binary refuses an old one. Link to the runbook.
- **`CONTRIBUTING.md`:** dev setup with Cargo and `frontend/`, the test gate, coverage, migrations through SeaORM with a new SQL file plus regenerated entities, an architecture section listing the crates and how Python's modules map onto them, and building with the embedded frontend.
- **`.env.example`:** token generation with `openssl rand -base64 32`, `postgresql://` URLs, the new settings.
- **`SECURITY.md`:** review for accuracy.
- **`docs/adr/0002-queued-ingest.md`:** leave the decision text alone. Add one line noting that "web workers" became replicas in ADR 0010.
- **`docs/adr/0010-rust-rewrite.md`:** check it still matches what was built, and correct it if not.
- **`docs/cutover-v2.md`:** the runbook below.

### Runbook: `docs/cutover-v2.md`

Written for the operator, in this order:

**Before merge**
1. Tag the last Python commit: `git tag v1.0.0 97b933d && git push origin v1.0.0`.

**At cutover**
2. Stop the Python deployment. CI uploads during the outage are lost, and the report actions do not fail builds.
3. Close every open issue labelled `flakeradar` in each ingested Repo (lawncare-saas, womens-fantasy-sports, writers-app). A fresh database has no links, and the filer does not search GitHub, so any Test that is still flaky would otherwise get a second issue. List them with `gh issue list --label flakeradar --state open --repo <repo>`.
4. Create an empty database and point `FLAKERADAR_DATABASE_URL` at it. Change any `?ssl=` to `?sslmode=`.
5. Deploy with `FLAKERADAR_GITHUB_TOKEN` **unset**.
6. Confirm `/api/health/ready`, a CI upload reaching `processed`, and the dashboard.
7. Check Fabro and writers-app still report and read successfully.

**After cutover**
8. Expect Fabro to treat known-flaky failures as new until history rebuilds (ADR 0028 in `fabro-workflows`): red checks go to `ci_fix` instead of a rerun. All Test and Job ids also changed.
9. Once scores have rebuilt (about a week of CI runs on the default branch), set `FLAKERADAR_GITHUB_TOKEN` again.

**Fabro**
10. In `fabro-workflows/ops/profile-images`, add `Dockerfile.rust-node-pg`, based on `Dockerfile.rust-node`, without the GTK and WebKit packages: Rust 1.99, Node 22, `cargo-nextest` 0.9.151, `cargo-audit`, PostgreSQL 17 and `fabro-pg-ensure`. Add `rust-node-pg:flakeradar:fabro-rust-node-pg:local` to `PROFILES` in `build-images.sh`. `build-images.sh` clones `main` and runs `.fabro/setup.sh` offline, so this works only after this PR merges.
11. Point FlakeRadar's Fabro automations at the new environment, and confirm a trivial run passes `.fabro/ci.sh`.

**Release**
12. Tag `v2.0.0` on the merge commit, and confirm `release.yml` publishes five binaries and `docker-publish.yml` publishes the image.

### Follow-on coverage issues

For each entry left in `coverage-gaps.md`, draft one GitHub issue with the file, function, reason and what would cover it. Save the drafts in `docs/plans/rust-rewrite/coverage-follow-ups.md`, and list them in the PR. The user files them after merge, because they must not carry the `agent` label until this PR is on `main`.

### The PR

Invoke the `pr` skill, then open one PR from `feat/rust-rewrite` to `main`:

- **Title:** `feat: replace the Python backend with one Rust binary`
- **Body:**
  - what changed and why, linking ADR 0010
  - that it needs a fresh install, linking the runbook
  - `differences.md`
  - final coverage figures, and the follow-ups
  - performance results
  - what was tested on the five targets
  - a short guide to reviewing it, in the order of the task docs
  - ends with the attribution line given to you for pull requests

Update `STATUS.md` before opening it. Update `CONTEXT.md` only if a domain term actually changed. None are expected to.

## Done when

- [ ] No Python remains except the two samples and the samples test.
- [ ] Every CI job is green on the PR, including all five targets and the coverage gate.
- [ ] The README quick start works from a clean machine state, using a binary from this branch's artifacts.
- [ ] The PR is open, and you have told the user it is ready, with the coverage figures and any open items.
