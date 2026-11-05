"""Record what the frozen Python backend does, once, as committed golden files.

The Rust rewrite replays these files (task 04 onwards) so Python never has to
run in CI. The task doc is `docs/plans/rust-rewrite/02-golden-recorder.md`; the
replay contract is `tests/parity/README.md`.

What it writes, all under `tests/parity/`:

* `golden/junit/<case>.line_base<0|1>.json` — the result of
  `app.parsing.parse_junit_xml` for every file in `junit/`, or the error.
* `golden/<scenario>/<nn>.json` — one file per step of every scenario in
  `scenarios/`, holding the status, the relevant headers and the parsed body.
* `schema.sql` — `pg_dump --schema-only --no-owner --no-privileges` of a
  database at Alembic head, with the `alembic_version` table removed.

It also fails when a route in FastAPI's own route table appears in no scenario.

Usage, from the repo root, with the backend virtualenv:

    backend/.venv/bin/python tools/parity-record/record.py          # rewrite the goldens
    backend/.venv/bin/python tools/parity-record/record.py --verify # fail on any diff

Docker must be running: each run starts a throwaway PostgreSQL 17 and one
uvicorn process per scenario.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.parse
from datetime import date, datetime
from pathlib import Path

import httpx

REPO_ROOT = Path(__file__).resolve().parents[2]
BACKEND_ROOT = REPO_ROOT / "backend"
PARITY_ROOT = REPO_ROOT / "tests" / "parity"
JUNIT_DIR = PARITY_ROOT / "junit"
SCENARIO_DIR = PARITY_ROOT / "scenarios"
GOLDEN_DIR = PARITY_ROOT / "golden"
SCHEMA_FILE = PARITY_ROOT / "schema.sql"
ALLOWED_FILE = PARITY_ROOT / "allowed-differences.json"
DIFFERENCES_FILE = REPO_ROOT / "docs" / "plans" / "rust-rewrite" / "differences.md"
FRONTEND_DIST = REPO_ROOT / "frontend" / "dist"
FRONTEND_DIST_ASIDE = REPO_ROOT / "frontend" / "dist.parity-hidden"

# A known token, so every golden records the same Authorization/X-API-Key values.
TOKEN = "parity-token"
# Written where the server filled in a value from the clock. The README defines
# what each one matches when a golden is replayed.
TIMESTAMP = "<timestamp>"
DATE = "<date>"

# Settings pinned for every scenario, so nothing leaks in from the caller's
# shell or a developer's backend/.env. A scenario's "settings" overrides these.
FIXED_SETTINGS = {
    "FLAKERADAR_API_TOKEN": TOKEN,
    "FLAKERADAR_WORKER_POLL_SECONDS": "0.05",
    "FLAKERADAR_PRUNE_INTERVAL_SECONDS": "3600",
    "FLAKERADAR_WEBHOOK_URLS": "",
    "FLAKERADAR_WEBHOOK_FORMAT": "json",
    "FLAKERADAR_GITHUB_TOKEN": "",
    "FLAKERADAR_GITHUB_SYNC_BATCH": "2000",
    "FLAKERADAR_PUBLIC_URL": "",
    "FLAKERADAR_SCORE_WINDOW": "50",
    "FLAKERADAR_SCORE_DECAY": "0.85",
    "FLAKERADAR_BROKEN_STREAK": "5",
    "FLAKERADAR_FLAKE_THRESHOLD": "0.3",
    "FLAKERADAR_GITHUB_ISSUE_MIN_SCORE": "0.3",
    "FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES": "0",
    "FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES": "0",
    "FLAKERADAR_GITHUB_FIXED_STREAK": "0",
    "FLAKERADAR_GITHUB_CLOSE_FIXED": "false",
    "FLAKERADAR_CORS_ORIGINS": "http://localhost:5173",
    "FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS": "30",
    "FLAKERADAR_MAX_REPORT_RETRIES": "3",
    "FLAKERADAR_REPORT_RETENTION_DAYS": "7",
    "FLAKERADAR_FAILED_REPORT_RETENTION_DAYS": "0",
    "FLAKERADAR_EXECUTION_RETENTION_DAYS": "90",
    "FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS": "365",
    "FLAKERADAR_PRUNE_BATCH": "5000",
    "FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS": "60",
}

# The timestamp rule from 00-shared-context.md: YYYY-MM-DDTHH:MM:SS, an optional
# fraction, then Z or +00:00.
TIMESTAMP_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]00:00)$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")

# Timestamps also arrive inside strings: an MCP tool result carries its payload
# as one JSON text block, so the clock-generated values are substituted in place
# rather than replaced whole.
EMBEDDED_TIMESTAMP_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]00:00)")
EMBEDDED_DATE_RE = re.compile(r"\d{4}-\d{2}-\d{2}")

# Response headers worth comparing: everything else (content-length, date,
# server) is framework noise.
RECORDED_HEADERS = ("content-type", "location")


def die(message: str) -> None:
    print(f"record.py: {message}", file=sys.stderr)
    sys.exit(2)


# ── Docker / PostgreSQL ───────────────────────────────────────────────────


class Postgres:
    """A throwaway PostgreSQL 17 container, plus the three commands we need."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.container = ""
        self.port = 0

    def start(self) -> None:
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True, check=False)
        run = subprocess.run(
            [
                "docker",
                "run",
                "--detach",
                "--rm",
                "--name",
                self.name,
                "-e",
                "POSTGRES_USER=flakeradar",
                "-e",
                "POSTGRES_PASSWORD=flakeradar",
                "-e",
                "POSTGRES_DB=flakeradar",
                "-p",
                "127.0.0.1::5432",
                "postgres:17-alpine",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        if run.returncode != 0:
            die(f"could not start PostgreSQL: {run.stderr.strip()}")
        self.container = run.stdout.strip()
        # The image's entrypoint runs a temporary server to create the cluster,
        # then restarts it, so one successful connection is not enough: wait for
        # two, a second apart.
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if self.exec(["psql", "-U", "flakeradar", "-d", "flakeradar", "-tAc", "SELECT 1"]).returncode == 0:
                time.sleep(1.0)
                if self.exec(["psql", "-U", "flakeradar", "-d", "flakeradar", "-tAc", "SELECT 1"]).returncode == 0:
                    break
            time.sleep(0.2)
        else:
            die("PostgreSQL did not become ready")
        printed = subprocess.run(
            ["docker", "port", self.container, "5432/tcp"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        self.port = int(printed.rsplit(":", 1)[1])

    def exec(self, command: list[str], env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
        options = [f"-e={key}={value}" for key, value in (env or {}).items()]
        return subprocess.run(
            ["docker", "exec", *options, self.container, *command],
            capture_output=True,
            text=True,
            check=False,
        )

    def psql(self, database: str, sql: str) -> str:
        run = self.exec(
            ["psql", "-U", "flakeradar", "-d", database, "-tAc", sql],
            env={"PGPASSWORD": "flakeradar"},
        )
        if run.returncode != 0:
            die(f"psql failed in {database}: {run.stderr.strip()}")
        return run.stdout

    def database_url(self, database: str) -> str:
        return f"postgresql+asyncpg://flakeradar:flakeradar@127.0.0.1:{self.port}/{database}"

    def fresh_database(self, database: str) -> None:
        self.psql("postgres", f'DROP DATABASE IF EXISTS "{database}" WITH (FORCE)')
        self.psql("postgres", f'CREATE DATABASE "{database}"')

    def schema_dump(self, database: str) -> str:
        run = self.exec(
            [
                "pg_dump",
                "-U",
                "flakeradar",
                "-d",
                database,
                "--schema-only",
                "--no-owner",
                "--no-privileges",
            ],
            env={"PGPASSWORD": "flakeradar"},
        )
        if run.returncode != 0:
            die(f"pg_dump failed: {run.stderr.strip()}")
        return run.stdout

    def stop(self) -> None:
        if self.container:
            subprocess.run(["docker", "rm", "-f", self.container], capture_output=True, check=False)


# ── The backend server ────────────────────────────────────────────────────


class HiddenFrontendDist:
    """Move frontend/dist aside for the run, then put it back.

    app.main mounts the built frontend at "/" when frontend/dist exists, which
    turns a trailing-slash request into a 404 from the static files instead of
    Starlette's 307. A fresh checkout has no dist, so the recorded behaviour
    would otherwise depend on whether someone ran `npm run build`.
    """

    def __init__(self) -> None:
        self.hidden = False

    def __enter__(self) -> "HiddenFrontendDist":
        if FRONTEND_DIST_ASIDE.exists():
            # A previous run died before restoring it.
            if not FRONTEND_DIST.exists():
                FRONTEND_DIST_ASIDE.rename(FRONTEND_DIST)
            else:
                shutil.rmtree(FRONTEND_DIST_ASIDE)
        if FRONTEND_DIST.is_dir():
            FRONTEND_DIST.rename(FRONTEND_DIST_ASIDE)
            self.hidden = True
            print("record.py: moved frontend/dist aside for the run", file=sys.stderr)
        return self

    def __exit__(self, *exc_info: object) -> None:
        if self.hidden and FRONTEND_DIST_ASIDE.is_dir():
            FRONTEND_DIST_ASIDE.rename(FRONTEND_DIST)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Server:
    """One uvicorn process against one database: migrations run at startup."""

    def __init__(self, database_url: str, settings: dict[str, str]) -> None:
        self.database_url = database_url
        self.settings = settings
        self.port = 0
        self.process: subprocess.Popen | None = None
        self.log = tempfile.TemporaryFile(mode="w+b")

    def start(self) -> None:
        self.port = free_port()
        # Every FLAKERADAR_* setting is pinned (or cleared) so a stray .env or
        # exported variable cannot change what is recorded.
        env = {key: value for key, value in os.environ.items() if not key.startswith("FLAKERADAR_")}
        env.update(FIXED_SETTINGS)
        env.update(self.settings)
        env["FLAKERADAR_DATABASE_URL"] = self.database_url
        env["PYTHONHASHSEED"] = "0"
        env["PYTHONUNBUFFERED"] = "1"
        self.process = subprocess.Popen(
            [
                sys.executable,
                "-m",
                "uvicorn",
                "app.main:app",
                "--host",
                "127.0.0.1",
                "--port",
                str(self.port),
                "--log-level",
                "warning",
            ],
            cwd=BACKEND_ROOT,
            env=env,
            stdout=self.log,
            stderr=self.log,
        )
        deadline = time.monotonic() + 120
        with httpx.Client(base_url=self.base_url, timeout=10.0) as client:
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    self._dump_log("the server exited during startup")
                try:
                    if client.get("/api/health").status_code == 200:
                        return
                except httpx.HTTPError:
                    pass
                time.sleep(0.1)
        self._dump_log("the server did not answer /api/health")

    def _dump_log(self, message: str) -> None:
        self.log.seek(0)
        text = self.log.read().decode("utf-8", "replace")
        die(f"{message}:\n{text[-4000:]}")

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=30)


# ── Normalization ─────────────────────────────────────────────────────────


def scenario_literals(value, keep: set[str]) -> set[str]:
    """Every timestamp or date string the scenario itself wrote."""
    if isinstance(value, dict):
        for item in value.values():
            scenario_literals(item, keep)
    elif isinstance(value, list):
        for item in value:
            scenario_literals(item, keep)
    elif isinstance(value, str) and (TIMESTAMP_RE.match(value) or DATE_RE.match(value)):
        keep.add(value)
    return keep


def normalize(value, keep: set[str]):
    """Replace clock-generated timestamps and dates with their placeholders."""
    if isinstance(value, dict):
        return {key: normalize(item, keep) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize(item, keep) for item in value]
    if isinstance(value, str) and value not in keep:
        if TIMESTAMP_RE.match(value):
            datetime.fromisoformat(value.replace("Z", "+00:00"))
            return TIMESTAMP
        if DATE_RE.match(value):
            date.fromisoformat(value)
            return DATE
        return _substitute_embedded(value, keep)
    return value


def _substitute_embedded(text: str, keep: set[str]) -> str:
    def replace_timestamp(match: re.Match) -> str:
        found = match.group(0)
        if found in keep:
            return found
        datetime.fromisoformat(found.replace("Z", "+00:00"))
        return TIMESTAMP

    def replace_date(match: re.Match) -> str:
        found = match.group(0)
        if found in keep:
            return found
        date.fromisoformat(found)
        return DATE

    return EMBEDDED_DATE_RE.sub(replace_date, EMBEDDED_TIMESTAMP_RE.sub(replace_timestamp, text))


# ── The JUnit corpus ──────────────────────────────────────────────────────


def backend_imports() -> tuple:
    """The frozen app's pure parsing functions, imported from backend/."""
    sys.path.insert(0, str(BACKEND_ROOT))
    os.environ.setdefault("FLAKERADAR_API_TOKEN", TOKEN)
    from app.classify import classify_failure
    from app.parsing import ParseError, fingerprint, parse_junit_xml

    return ParseError, fingerprint, parse_junit_xml, classify_failure


def record_junit() -> dict[Path, str]:
    """Parse every corpus file with both line_base values under the frozen app."""
    ParseError, fingerprint, parse_junit_xml, classify_failure = backend_imports()
    out: dict[Path, str] = {}
    for source in sorted(JUNIT_DIR.glob("*.xml")):
        for line_base in (1, 0):
            record: dict[str, object] = {"source": f"junit/{source.name}", "line_base": line_base}
            try:
                cases = parse_junit_xml(source.read_bytes(), line_base)
            except ParseError as exc:
                record["error"] = str(exc)
            except Exception as exc:  # noqa: BLE001 - record what Python really does
                # Not a handled ParseError: the frozen code raises out of
                # parse_junit_xml (and so out of POST /api/ingest as a 500).
                # Recorded verbatim, with a marker that it was unhandled.
                record["error"] = f"unhandled {type(exc).__name__}: {exc}"
            else:
                record["cases"] = [
                    {
                        "suite": case.suite,
                        "classname": case.classname,
                        "name": case.name,
                        "status": case.status,
                        "duration": case.duration,
                        "message": case.message,
                        "details": case.details,
                        "file": case.file,
                        "line": case.line,
                        "fingerprint": fingerprint(case.suite, case.classname, case.name),
                        "category": classify_failure(case.message, case.details)
                        if case.status in ("failed", "error")
                        else None,
                    }
                    for case in cases
                ]
            target = GOLDEN_DIR / "junit" / f"{source.stem}.line_base{line_base}.json"
            out[target] = dumps(record)
    return out


def dumps(value) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# ── Scenarios ─────────────────────────────────────────────────────────────


class Recorder:
    """Runs one scenario against one fresh database and records each step."""

    def __init__(self, scenario: dict, server: Server, database: str) -> None:
        self.scenario = scenario
        self.server = server
        self.database = database
        self.keep = scenario_literals(scenario, set())
        # One connection per request: a 500 closes the connection server-side,
        # and a reused one would fail the next request instead of recording it.
        self.client = httpx.Client(
            base_url=server.base_url, timeout=120.0, headers={"Connection": "close"}
        )

    def close(self) -> None:
        self.client.close()

    def run(self) -> dict[Path, str]:
        out: dict[Path, str] = {}
        name = self.scenario["name"]
        for number, step in enumerate(self.scenario["steps"], start=1):
            try:
                record = self.step(step)
            except Exception as exc:
                die(f"{name} step {number} ({sorted(step)}): {type(exc).__name__}: {exc}")
            header = {"step": number}
            header.update(record)
            out[GOLDEN_DIR / name / f"{number:02d}.json"] = dumps(normalize(header, self.keep))
        return out

    def step(self, step: dict) -> dict:
        if "http" in step:
            return self.http_step(step["http"])
        if "wait_processed" in step:
            return self.wait_processed()
        if "dump_db" in step:
            return self.dump_db(step["dump_db"])
        if "mcp" in step:
            return self.mcp_step(step["mcp"], step.get("database"))
        raise ValueError(f"unknown step type: {sorted(step)}")

    def http_step(self, spec: dict) -> dict:
        method = spec["method"]
        path = spec["path"]
        query = {key: self.substitute(value) for key, value in spec.get("query", {}).items()}
        headers = {key: self.substitute(value) for key, value in spec.get("headers", {}).items()}
        if "multipart_file" in spec:
            upload = spec["multipart_file"]
            # httpx sets the multipart Content-Type, boundary included.
            files = {upload["field"]: (upload["filename"], (PARITY_ROOT / upload["body_file"]).read_bytes())}
            response = self.client.request(method, path, params=query, headers=headers, files=files)
        else:
            response = self.client.request(method, path, params=query, headers=headers, content=self.body(spec))
        record: dict = {
            "request": {"method": method, "path": path, "query": query},
            "status": response.status_code,
        }
        for header in RECORDED_HEADERS:
            if header not in response.headers:
                continue
            value = response.headers[header]
            if header == "location":
                # The server answers with an absolute URL; only the target it
                # points at is part of the contract, and the port is not stable.
                parsed = urllib.parse.urlsplit(value)
                value = parsed.path + (f"?{parsed.query}" if parsed.query else "")
            record[header] = value
        record["body"] = decode_body(response)
        return record

    def body(self, spec: dict) -> bytes | None:
        if "body_file" in spec:
            return (PARITY_ROOT / spec["body_file"]).read_bytes()
        if "body_json" in spec:
            return json.dumps(spec["body_json"]).encode()
        if "body_text" in spec:
            return spec["body_text"].encode()
        if "body_size" in spec:
            # A filler body of exactly this many bytes, for size-limit cases.
            return b"a" * spec["body_size"]
        return None

    def substitute(self, value):
        return value.replace("$TOKEN", TOKEN) if isinstance(value, str) else value

    def wait_processed(self) -> dict:
        """Block until the worker has drained the queue, then record its state."""
        deadline = time.monotonic() + 300
        summary: dict = {}
        while time.monotonic() < deadline:
            summary = self.client.get("/api/reports/summary").json()
            if summary.get("pending") == 0:
                return {"processed": True, "reports": summary}
            time.sleep(0.05)
        die(f"the queue did not drain: {summary}")

    def dump_db(self, tables: list) -> dict:
        """Every row of the named tables, ordered so the dump is reproducible.

        An entry is a table name (ordered by id) or {"table", "order_by"} for a
        table whose rows have no id, such as the Score history tables.
        """
        dump = {}
        for entry in tables:
            table = entry if isinstance(entry, str) else entry["table"]
            order = "id" if isinstance(entry, str) else entry["order_by"]
            sql = f"SELECT coalesce(json_agg(t), '[]'::json) FROM (SELECT * FROM {table} ORDER BY {order}) t"
            dump[table] = json.loads(self.postgres.psql(self.database, sql))
        return {"tables": dump}

    def mcp_step(self, spec: dict, database: str | None = None) -> dict:
        from fastmcp import Client
        from fastmcp.exceptions import ToolError

        url = f"{self.server.base_url}/mcp/"
        auth = None if spec.get("anonymous") else TOKEN

        async def call() -> dict:
            async with Client(url, auth=auth, timeout=30, mode="legacy") as client:
                if spec["call"] == "initialize":
                    return {"result": (await client.initialize()).model_dump(mode="json", by_alias=True)}
                if spec["call"] == "tools/list":
                    return {"tools": [tool.model_dump(mode="json", by_alias=True) for tool in await client.list_tools()]}
                if spec["call"] == "tools/call":
                    # The raw protocol result: content blocks, structuredContent
                    # and isError, exactly as a non-FastMCP client sees it.
                    result = await client.call_tool_mcp(spec["tool"], spec.get("arguments", {}))
                    return {"result": result.model_dump(mode="json", by_alias=True)}
                raise ValueError(f"unknown mcp call {spec['call']!r}")

        try:
            return asyncio.run(call())
        except ToolError as exc:
            return {"error": {"type": "ToolError", "message": str(exc)}}
        except Exception as exc:  # noqa: BLE001 - protocol-level failures are recorded, not raised
            return {"error": {"type": type(exc).__name__, "message": str(exc)}}


def decode_body(response: httpx.Response):
    if "application/json" in response.headers.get("content-type", ""):
        return response.json()
    return response.text


# ── Route coverage ────────────────────────────────────────────────────────

# Paths FastAPI serves itself. The Rust binary replaces the docs UI (Redoc at
# /docs, /redoc gone — see differences.md) and mounts MCP at /mcp/, so these
# are not part of the scenario coverage this script requires.
NON_SCENARIO_PATHS = {"/openapi.json", "/docs", "/docs/oauth2-redirect", "/redoc"}


def route_table() -> list[tuple[str, str]]:
    sys.path.insert(0, str(BACKEND_ROOT))
    os.environ.setdefault("FLAKERADAR_API_TOKEN", TOKEN)
    from app.main import app

    routes: list[tuple[str, str]] = []
    for path, operations in app.openapi()["paths"].items():
        if path in NON_SCENARIO_PATHS:
            continue
        for method in operations:
            if method in ("get", "post", "put", "delete", "patch"):
                routes.append((method.upper(), path))
    return routes


def path_matches(template: str, path: str) -> bool:
    """True when a concrete request path fills a FastAPI path template."""
    pattern = re.sub(r"\\\{[^}]+\\\}", "[^/]+", re.escape(template))
    return re.match(f"^{pattern}$", path) is not None


def missing_routes(scenarios: list[dict]) -> list[str]:
    seen = [
        (step["http"]["method"].upper(), step["http"]["path"])
        for scenario in scenarios
        for step in scenario["steps"]
        if "http" in step
    ]
    return [
        f"{method} {template}"
        for method, template in route_table()
        if not any(method == candidate and path_matches(template, path) for candidate, path in seen)
    ]


# ── schema.sql ────────────────────────────────────────────────────────────


def strip_alembic(dump: str) -> str:
    """Drop every statement that mentions alembic_version from a pg_dump.

    The Rust binary never reads alembic_version: a fresh install has none, and
    the baseline schema is what task 03 turns into a SeaORM migration.
    """
    kept: list[str] = []
    for statement in dump.split(";\n"):
        if "alembic_version" in statement:
            continue
        kept.append(statement)
    text = ";\n".join(kept)
    # pg_dump 17 wraps the script in \\restrict/\\unrestrict guards; they are
    # psql metadata, not schema.
    lines = [line for line in text.splitlines() if not line.startswith(("\\restrict", "\\unrestrict"))]
    return "\n".join(lines).lstrip("\n")


# ── Writing and verifying ─────────────────────────────────────────────────


def check_allowed_differences(files: dict[Path, str]) -> None:
    """Every allowance names a differences.md row and a field that exists.

    An allowance that points at a renamed golden, or a differences.md row
    rewritten without its allowance, is how this mechanism rots, so the recorder
    checks both directions it can see.

    One exception: an allowance that names a Rust value and no Python value
    records a field the answer *adds*, which the golden cannot carry. Every other
    allowance must name a field the golden has.
    """
    allowed = json.loads(ALLOWED_FILE.read_text(encoding="utf-8"))
    differences = DIFFERENCES_FILE.read_text(encoding="utf-8")
    problems: list[str] = []
    for entry in allowed:
        if entry["differenceRow"] not in differences:
            problems.append(f"{entry['id']}: no differences.md row {entry['differenceRow']!r}")
        for allowance in entry["allows"]:
            content = files.get(PARITY_ROOT / allowance["file"])
            if content is None:
                problems.append(f"{entry['id']}: {allowance['file']} is not a recorded golden")
                continue
            node = json.loads(content)
            missing = False
            for part in allowance["field"].split("."):
                if isinstance(node, list):
                    index = int(part) if part.isdigit() else -1
                    if not 0 <= index < len(node):
                        missing = True
                        break
                    node = node[index]
                elif isinstance(node, dict) and part in node:
                    node = node[part]
                else:
                    missing = True
                    break
            if missing and not ("rustValue" in allowance and "pythonValue" not in allowance):
                problems.append(f"{entry['id']}: {allowance['file']} has no field {allowance['field']}")
    if problems:
        die("allowed-differences.json is stale:\n  " + "\n  ".join(problems))


def write_or_verify(files: dict[Path, str], verify: bool) -> list[str]:
    """Write the files, or compare them with what is committed."""
    if not verify:
        for target, content in files.items():
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        return []

    failures: list[str] = []
    committed = {
        path.relative_to(PARITY_ROOT)
        for pattern in ("golden/**/*.json", "schema.sql")
        for path in PARITY_ROOT.glob(pattern)
    }
    produced = {target.relative_to(PARITY_ROOT) for target in files}
    for stale in sorted(committed - produced):
        failures.append(f"stale: tests/parity/{stale}")
    for target, content in files.items():
        relative = target.relative_to(PARITY_ROOT)
        if not (PARITY_ROOT / relative).is_file():
            failures.append(f"new: tests/parity/{relative}")
        elif (PARITY_ROOT / relative).read_text(encoding="utf-8") != content:
            failures.append(f"differs: tests/parity/{relative}")
    return failures


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Record or verify the parity goldens.")
    parser.add_argument("--verify", action="store_true", help="compare with the committed goldens; write nothing")
    parser.add_argument(
        "--only",
        action="append",
        choices=("junit", "scenarios", "schema"),
        help="record only this part (repeatable)",
    )
    parser.add_argument(
        "--scenario",
        action="append",
        help="record only these scenarios (repeatable); the route-coverage check then waits "
        "for a full run, because it needs every scenario",
    )
    args = parser.parse_args(argv)
    only = set(args.only) if args.only else {"junit", "scenarios", "schema"}

    files: dict[Path, str] = {}
    scenarios: list[dict] = []
    if "scenarios" in only or "schema" in only:
        for path in sorted(SCENARIO_DIR.glob("*.json")):
            scenarios.append(json.loads(path.read_text(encoding="utf-8")))
        names = [scenario["name"] for scenario in scenarios]
        if len(set(names)) != len(names):
            die("two scenarios share a name")
        if "junit" in names:
            die("no scenario may be called 'junit': it owns tests/parity/golden/junit/")
        if args.scenario:
            known = set(names)
            unknown = sorted(set(args.scenario) - known)
            if unknown:
                die("no such scenario: " + ", ".join(unknown))
            scenarios = [scenario for scenario in scenarios if scenario["name"] in set(args.scenario)]
        else:
            missing = missing_routes(scenarios)
            if missing:
                die("no scenario calls: " + ", ".join(missing))

    if "junit" in only:
        files.update(record_junit())

    with HiddenFrontendDist():
        files.update(record_served(scenarios if ("scenarios" in only or "schema" in only) else [], only))

    if "scenarios" in only or "junit" in only:
        check_allowed_differences(files)

    failures = write_or_verify(files, args.verify)
    if failures:
        print("Parity goldens are out of date:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print(f"{'Verified' if args.verify else 'Recorded'} {len(files)} files.")
    return 0


def record_served(scenarios: list[dict], only: set[str]) -> dict[Path, str]:
    """The goldens that need a running server: the scenarios and schema.sql."""
    files: dict[Path, str] = {}
    postgres = Postgres("flakeradar-parity-record")
    try:
        postgres.start()
        if "scenarios" in only:
            for scenario in scenarios:
                database = f"parity_{re.sub(r'[^a-z0-9_]', '_', scenario['name'])}"
                postgres.fresh_database(database)
                server = Server(postgres.database_url(database), scenario.get("settings", {}))
                server.start()
                try:
                    recorder = Recorder(scenario, server, database)
                    recorder.postgres = postgres
                    try:
                        files.update(recorder.run())
                    finally:
                        recorder.close()
                finally:
                    server.stop()
        if "schema" in only:
            database = "parity_schema"
            postgres.fresh_database(database)
            server = Server(postgres.database_url(database), {})
            server.start()
            server.stop()
            files[SCHEMA_FILE] = strip_alembic(postgres.schema_dump(database))
    finally:
        postgres.stop()
    return files


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
