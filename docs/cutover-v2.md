# Cutover to FlakeRadar 2.0.0 (the Rust binary)

This is the operator's runbook for replacing the Python server (1.x) with the
single Rust binary. FlakeRadar 2.0 needs a **fresh database**: there is no
import tool and the binary refuses to start on a database that Alembic created,
or on one holding FlakeRadar tables with no `seaql_migrations` row. It never
drops or changes anything; it stops with
`this database was not created by FlakeRadar 2: it holds an alembic_version table`.

Read step 8 before you start: FlakeRadar's own Test and Job ids change, and
Fabro will treat known-flaky failures as new until history rebuilds.

## Before merge

### 1. Tag the last Python commit

`97b933d` is the last commit whose server was Python. It is the reference the
parity goldens were recorded from.

```bash
git tag v1.0.0 97b933d && git push origin v1.0.0
```

1.x stays downloadable from that tag, so a regression has a way back.

## At cutover

### 2. Stop the Python deployment

Stop the old container or service before you point anything at the new binary.
CI uploads during the outage are **lost**: the report actions do not fail a
build when the upload fails, so nothing will tell you which Reports were
dropped. They cannot be recovered from the old database either, because the new
one starts empty. If you need a quiet window, stop the deployments of the
repositories that report to FlakeRadar first, then the server.

### 3. Close every open `flakeradar` issue in each ingested Repo

A fresh database has no Issue links, and the filer does not search GitHub before
it files. Any Test that is still flaky at cutover would therefore get a second
issue for the same problem. List and close them:

```bash
for repo in lawncare-saas womens-fantasy-sports writers-app; do
  gh issue list --label flakeradar --state open --repo "$repo"
done
# then close each one, e.g.
gh issue close <number> --repo <repo> --reason "not planned"
```

The label is `FLAKERADAR_GITHUB_ISSUE_LABEL` (default `flakeradar`); substitute
yours if you changed it. Closing is safe to do before the new server runs: the
new database would not have known about those issues anyway.

### 4. Create an empty database and point the binary at it

Outside the shipped `docker-compose.yml` (for compose, see below):

```sql
CREATE DATABASE flakeradar;
```

```bash
# .env next to the binary, or the container's environment
FLAKERADAR_DATABASE_URL=postgresql://flakeradar:<password>@<host>:5432/flakeradar
```

With the shipped `docker-compose.yml`, the 1.x database is already called
`flakeradar` in the `flakeradar-pg` volume, and compose builds the URL itself,
so a `FLAKERADAR_DATABASE_URL` in `.env` is ignored. Create the new database
under another name instead and name it with `POSTGRES_DB` in `.env`:

```bash
docker compose up -d db
docker compose exec db createdb -U flakeradar flakeradar2
printf '\nPOSTGRES_DB=flakeradar2\n' >> .env
```

The old database stays beside it, untouched, as the way back to 1.x.

If the old URL carried `?ssl=` query parameters, change them to `?sslmode=`.
The Rust driver reads libpq's names, and `ssl=` is a SQLAlchemy spelling it
ignores. The binary creates its whole schema on startup (`seaql_migrations`
plus the baseline migration), so nothing else has to be seeded or migrated.

### 5. Deploy with `FLAKERADAR_GITHUB_TOKEN` unset

Start the new binary with the token **unset**, for two reasons: the first
scores are computed from one upload, so the filing gate would otherwise file
issues on almost no evidence, and the sync would have nothing to check. Set it
again in step 9.

```bash
FLAKERADAR_API_TOKEN="$(openssl rand -base64 32)" \
FLAKERADAR_DATABASE_URL=postgresql://... \
./flakeradar
# or: docker run --env-file .env -p 127.0.0.1:8000:8000 <image>
```

The binary binds `127.0.0.1:8000` unless `FLAKERADAR_HOST`/`FLAKERADAR_PORT` say
otherwise; the image sets `0.0.0.0`. Keep the dashboard on a private network:
the read APIs and the quarantine toggle are unauthenticated by design.

### 6. Confirm the server serves

```bash
curl -fsS http://127.0.0.1:8000/api/health/ready
# {"database":true,"processor_leader":true,"pending_reports":0,...}
```

- `database: true` means the schema is there and a query works.
- `processor_leader: true` means this process holds the report-processor lock.
- `pending_reports` must go to `0` after the next upload.

Then make one real upload through a repository's normal `actions/report` (or
`curl`) and confirm the Report reaches `processed`:

```bash
curl -fsS -H "X-API-Key: $FLAKERADAR_API_TOKEN" \
  "http://127.0.0.1:8000/api/reports/<id>"
# .status == "processed", .counts holds the Executions
```

Finally load the dashboard at `/` and check the leaderboard shows that Run.
`/docs` is the Redoc reference and needs no network access.

### 7. Check the two consumers that are not humans

- **Fabro** (`fabro-workflows`): a `flakeradar` read of `/api/tests` and
  `/api/ci-runs/{id}/failures` succeeds, and `PUT /api/tests/{id}/issue`
  returns a linked issue.
- **writers-app**: a `POST /api/ingest` with `Content-Type: application/xml`
  answers `202` and the Report reaches `processed`.

Both read the same field names as before; nothing in either client changed.

## After cutover

### 8. Expect known-flaky failures to look new for about a week

Fabro decides between `ci_fix` and a rerun from FlakeRadar's history (ADR 0028
in `fabro-workflows`). An empty database has no history, so every failure looks
new and red checks go to `ci_fix` instead of a rerun. This lasts until the
scores rebuild, which takes about a week of CI runs on the default branch.

Every Test id and Job id also changed, because ids come from the new database's
sequences. Anything outside FlakeRadar that stored an id — a Fabro run's notes,
a dashboard URL someone bookmarked, an issue body written by hand — points at
nothing now. FlakeRadar's own issue bodies do not carry ids, so no filed issue
needs editing.

### 9. Set `FLAKERADAR_GITHUB_TOKEN` again once scores have rebuilt

After about a week, the tiers are meaningful again. Set the token and restart:

```bash
FLAKERADAR_GITHUB_TOKEN=ghp_... ./flakeradar
```

The filer then opens issues for Tests that cross the gate, and the sync starts
checking the issues it files. Step 3 is why this is safe: there is no open
`flakeradar` issue for it to file a duplicate of.

## Fabro

### 10. Add the Rust + Node + PostgreSQL sandbox image

In `fabro-workflows/ops/profile-images`:

- Add `Dockerfile.rust-node-pg`, based on `Dockerfile.rust-node`, **without**
  the GTK and WebKit packages: Rust 1.99, Node 22, `cargo-nextest` 0.9.151,
  `cargo-audit`, PostgreSQL 17 and `fabro-pg-ensure`.
- Add `rust-node-pg:flakeradar:fabro-rust-node-pg:local` to `PROFILES` in
  `build-images.sh`.

This works only **after** the cutover PR merges: `build-images.sh` clones
`main` and runs `.fabro/setup.sh` offline, and `setup.sh` needs the Rust
workspace, which exists on `main` only once the PR is merged.

### 11. Point FlakeRadar's own Fabro automations at the new image

Set the environment of FlakeRadar's automations to the new profile, then
confirm a trivial run passes `.fabro/ci.sh`. The image matters because the
workspace has no Python to build: `setup.sh` runs `npm ci`, `cargo fetch` and
installs `cargo-audit`, and `ci.sh` runs rustfmt, clippy, nextest, `cargo audit`
and the frontend gates.

## Release

### 12. Tag `v2.0.0` on the merge commit

```bash
git tag v2.0.0 <merge-commit> && git push origin v2.0.0
```

Then confirm both workflows finished:

```bash
gh run list --workflow release.yml --limit 1
gh run list --workflow docker-publish.yml --limit 1
```

`release.yml` publishes five binaries — `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin` and
`x86_64-pc-windows-msvc`, each with a checksum and a build-provenance
attestation — and `docker-publish.yml` publishes the image. Windows on ARM is
not a target.

Verify a download the way the README describes (Linux and macOS get a
`.tar.gz`, Windows a `.zip`, each with a `.sha256` beside it):

```bash
gh attestation verify flakeradar-2.0.0-x86_64-unknown-linux-musl.tar.gz \
  --repo andrewthetechie/flakeradar
sha256sum -c flakeradar-2.0.0-x86_64-unknown-linux-musl.tar.gz.sha256
```
