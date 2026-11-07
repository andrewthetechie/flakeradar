# 13 — CLI, Docker, release, five-target smoke test

Read `00-shared-context.md` first. Tasks 01–12 have landed.

Reference: `backend/Dockerfile`, `docker-compose.yml`, `.github/workflows/docker-publish.yml`, `writers-app/.github/workflows/release.yml`.

## Goal

Real binaries for all five targets, proven to run, plus a slim image.

## Deliverables

### CLI (`clap`)

- `flakeradar` with no arguments runs the server. `flakeradar serve` does the same.
- `flakeradar --version` prints the workspace version.
- `flakeradar healthcheck`: GET `http://127.0.0.1:$FLAKERADAR_PORT/api/health` with a 2-second timeout, loading settings the same way as the server (a wildcard or unset host still connects to loopback). Exit 0 on 200, otherwise 1. It needs no database and no token.
- Exit non-zero with a clear message on the default token, a bad setting, a bad URL or an Alembic database.

On musl targets, use `mimalloc` as the global allocator, because musl's allocator performs poorly under multi-threaded load.

### `scripts/smoke.sh`

Bash, working on Linux, macOS and Git Bash on Windows, used by CI. Given a binary path and a database URL, it:

1. Runs the binary from an empty temp directory.
2. Polls `/api/health/ready` until `database: true`.
3. POSTs `actions/report/test/fixture-alpha.xml` to `/api/ingest`.
4. Polls `/api/reports/{id}` until `processed`.
5. Checks `/api/tests` lists the Tests.
6. Checks `/` returns HTML with the root element, and an `/assets/` file loads.
7. Checks `/docs` and `/openapi.json`.
8. Calls MCP `tools/list` with the bearer token and gets 11 tools.
9. Runs `flakeradar healthcheck`, expecting exit 0.
10. Stops the process and confirms a clean exit.

Separately, it checks that the binary run with the default token and no opt-in exits non-zero.

### CI: `targets` job in `ci.yml`

- A `frontend-dist` job builds `frontend/dist` once and uploads it.
- Matrix, native builds on each, depending on `frontend-dist`:

| Target | Runner |
|---|---|
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` (install `musl-tools`) |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` (install `musl-tools`) |
| `x86_64-apple-darwin` | `macos-15-intel` |
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-pc-windows-msvc` | `windows-latest` |

- Each leg runs: `cargo test -p flakeradar-core`, `cargo build --release --locked --target ...`, `ikalnytskyi/action-setup-postgres` (it supports Linux, macOS and Windows; request Postgres 17 where offered), then `scripts/smoke.sh`.
- Windows: static CRT (`-C target-feature=+crt-static`).
- Linux: assert the binary is static with `ldd` (or `file`).
- Run on PRs and pushes when Rust, `frontend/`, the workflow or `scripts/` changed, using `dorny/paths-filter` as writers-app does. Always run on `feat/rust-rewrite`.
- Upload each binary as a build artifact.

### Release (`.github/workflows/release.yml`)

Modelled on writers-app's:

- Trigger on `v*` tags, plus `workflow_dispatch` that builds without publishing.
- Serialized with `concurrency`.
- Validate first: the tag is `vMAJOR.MINOR.PATCH` and `node scripts/version.mjs check <version>` passes.
- Build the same matrix and run the smoke test.
- Package as `flakeradar-<version>-<target>.tar.gz` (`.zip` on Windows), including `LICENSE` and `NOTICE`, with a `.sha256` for each.
- Use `actions/attest-build-provenance` for attestations.
- Resume-safe publishing: create the release if it is missing, and upload only absent assets. Never `--clobber`.
- macOS and Windows binaries are unsigned. Task 15 documents the prompts.

### Docker

- New root `Dockerfile`, replacing `backend/Dockerfile` in task 15:
  - stages: Node 22 builds `frontend`, then a Rust musl stage builds the binary with `--locked`
  - final stage `gcr.io/distroless/static-debian13:nonroot@sha256:<digest>`, with only the binary
  - `ENV FLAKERADAR_HOST=0.0.0.0`, `EXPOSE 8000`
  - exec-form `HEALTHCHECK` with `/flakeradar healthcheck`, keeping `--interval=30s --timeout=3s --start-period=15s --retries=3`
- `.dockerignore`: add `target`.
- `docker-compose.yml` builds from the root `Dockerfile`, keeps the `127.0.0.1:8000:8000` publish, and uses `postgresql://` in `FLAKERADAR_DATABASE_URL`.
- `.github/workflows/docker-publish.yml`: **drop QEMU**. Build each architecture natively on `ubuntu-24.04` and `ubuntu-24.04-arm`, push by digest, then merge with `docker buildx imagetools create`. Keep the tags and the GHCR image name. PRs build without pushing.

## Tests

The CLI's argument handling and `healthcheck` get unit tests with a local listener, covering 200, non-200, connection refused and a wildcard host. `scripts/smoke.sh` is proven by CI.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] All five `targets` legs are green on GitHub.
- [ ] `docker compose up --build` works from a clean checkout with a real token, the container reports healthy, and the dashboard loads.
- [ ] `docker image inspect` shows the image under 60 MB, and it runs as non-root.
- [ ] `release.yml` passes a `workflow_dispatch` build-only run.
- [ ] Gaps are logged, and `STATUS.md` is updated, and the branch is pushed.
