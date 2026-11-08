# Deploy FlakeRadar

FlakeRadar is one binary plus a PostgreSQL database. One process holds the
API, the report processor, the dashboard and the MCP server. On first start,
the binary creates its whole schema, so you do not seed or migrate anything.

You can run FlakeRadar in three ways:

- [Docker Compose](#docker-compose) runs FlakeRadar and PostgreSQL together.
- [The release binary](#the-release-binary) runs with a PostgreSQL database
  that you supply. Any PostgreSQL 12 or later works.
- [From source](#from-source) is for development.

After you deploy, read [Configuration](configuration.md) and
[Monitoring](monitoring.md).

## Network access

The read APIs and the quarantine toggle have no authentication. This is by
design. Put the dashboard on a private network.

The CI upload endpoints, the issue link endpoints and the MCP server need the
API token (`FLAKERADAR_API_TOKEN`). The server does not start with the default
token `changeme`. Generate a token:

```bash
openssl rand -base64 32
```

The bare binary binds `127.0.0.1:8000`. Set `FLAKERADAR_HOST` and
`FLAKERADAR_PORT` to change the address. The container image binds
`0.0.0.0:8000`.

## Docker Compose

Run these commands in a clone of this repository:

```bash
cp .env.example .env   # set FLAKERADAR_API_TOKEN and POSTGRES_PASSWORD
docker compose up --build
```

The app, the API and the MCP server are on `http://localhost:8000`. Compose
publishes the port on `127.0.0.1` only.

Compose runs two services:

- `db` is PostgreSQL 17. Its data is in the named volume `flakeradar-pg`.
- `flakeradar` is the API, the report processor and the built dashboard, in
  one process.

Compose builds `FLAKERADAR_DATABASE_URL` from `POSTGRES_PASSWORD` and
`POSTGRES_DB` in `.env`. It ignores a `FLAKERADAR_DATABASE_URL` in `.env`.
`POSTGRES_DB` is `flakeradar` by default.

To load demo data, run the sample script. It needs Python and the `httpx`
package:

```bash
python samples/simulate_ci.py http://localhost:8000 "$FLAKERADAR_API_TOKEN"
```

The script sends 14 CI runs of the repo `demo/shop`. Open
`http://localhost:8000/?repo=demo/shop` to see the result.

## The release binary

Each release has a binary for five targets:

| Target | Runs on | Archive |
|---|---|---|
| `x86_64-unknown-linux-musl` | Linux x86-64 (statically linked) | `.tar.gz` |
| `aarch64-unknown-linux-musl` | Linux ARM64 (statically linked) | `.tar.gz` |
| `x86_64-apple-darwin` | macOS on Intel | `.tar.gz` |
| `aarch64-apple-darwin` | macOS on Apple silicon | `.tar.gz` |
| `x86_64-pc-windows-msvc` | Windows x86-64 (static CRT) | `.zip` |

Windows on ARM64 is not a target. Its GitHub runner image has no PostgreSQL
for the smoke test, so no Windows ARM binary is built or released.

```bash
# 1. A database, if you do not have one.
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# 2. The binary for your platform, from the newest release (0.2.0 here).
TARGET=aarch64-apple-darwin      # your target, from the table above
gh release download v0.2.0 --repo andrewthetechie/flakeradar \
  --pattern "flakeradar-0.2.0-$TARGET.tar.gz"
tar -xzf "flakeradar-0.2.0-$TARGET.tar.gz"

# 3. Run it. On first start it creates its whole schema.
FLAKERADAR_API_TOKEN="$(openssl rand -base64 32)" \
FLAKERADAR_DATABASE_URL=postgresql://flakeradar:flakeradar@localhost:5432/flakeradar \
  ./flakeradar
# The app, the API and the MCP server are on http://127.0.0.1:8000.
```

### Verify the download

Each archive has a `.sha256` file beside it. Each archive also has a build
provenance attestation. Check both, to prove that the file is intact and that
GitHub Actions built it from this repository:

```bash
gh attestation verify flakeradar-0.2.0-x86_64-unknown-linux-musl.tar.gz \
  --repo andrewthetechie/flakeradar
sha256sum -c flakeradar-0.2.0-x86_64-unknown-linux-musl.tar.gz.sha256
# macOS: shasum -a 256 -c ...
```

### The first run on macOS and Windows

The binaries are not code-signed. The operating system shows a warning the
first time that you run one:

- macOS Gatekeeper: the browser marks a downloaded binary with a quarantine
  attribute. The first run stops with "Apple could not verify ... is free of
  malware". Allow the file one time in System Settings → Privacy & Security →
  "Open Anyway", or remove the attribute with
  `xattr -d com.apple.quarantine ./flakeradar`. Both methods apply to one file.
  You need one of them because the binary is not notarised.
- Windows SmartScreen: the first run shows "Windows protected your PC". Choose
  "More info" → "Run anyway". SmartScreen remembers the file after that.

Neither prompt replaces the attestation and checksum check. The check proves
that GitHub Actions built the file from this repository. The prompt only lets
you run it.

## From source

To build and run FlakeRadar from source, you need PostgreSQL, Cargo and Node:

```bash
docker run -d --name flakeradar-pg -p 5432:5432 \
  -e POSTGRES_USER=flakeradar -e POSTGRES_PASSWORD=flakeradar -e POSTGRES_DB=flakeradar \
  postgres:17-alpine

# The server embeds frontend/dist when it exists. Build the frontend first if
# you want the binary to serve the dashboard.
(cd frontend && npm install && npm run build)
FLAKERADAR_ALLOW_INSECURE=1 \
FLAKERADAR_DATABASE_URL=postgresql://flakeradar:flakeradar@localhost:5432/flakeradar \
  cargo run -p flakeradar     # http://127.0.0.1:8000

# Or run the frontend dev server, with hot reload. It sends /api requests to :8000.
cd frontend
npm run dev            # http://localhost:5173
```

[CONTRIBUTING.md](../CONTRIBUTING.md) has the test gate, the migration recipe
and the crate layout.

## More than one replica

You can run more than one replica against one database. Only the process that
holds the advisory lock of the report processor processes reports. The MCP
server is stateless, so no sticky routing is necessary.

## Upgrading from 0.1.x

FlakeRadar 0.1.x was the Python server. 0.2 is a fresh install. There is no
import tool and no data migration.

The 0.2 binary does not start on a database that the Python server created,
because that database has an `alembic_version` table. It also does not start
on a database with FlakeRadar tables and no `seaql_migrations` row. It never
drops or changes anything. Thus it is safe to point it at an old database.
Then create an empty database beside the old one.

Scores, tiers, proven flake counts and score history start empty. They rebuild
from new reports, which takes approximately one week of CI runs on the default
branch. Test and job ids change, so a stored id no longer identifies the same
test.

[cutover-rust.md](cutover-rust.md) is the step-by-step runbook. In summary:

1. Tag the last Python commit.
2. Stop the old deployment.
3. Close the open `flakeradar` issues.
4. Create the database.
5. Deploy with the GitHub token unset.
6. Set the token again after the scores rebuild.
