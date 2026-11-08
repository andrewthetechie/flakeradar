"""One request, both servers, one comparison.

Every case that the differential sweep can attribute to a recorded difference in
`differences.md` names that record with `known=` (and, for a field whose text is
allowed to differ, masks it with `mask=`). A case that names no record and still
differs is a NEW difference: the battery fails on it, and the fix belongs in the
port (or in a new `differences.md` row with the owner's approval).

Both servers are compared on the answer's status, on the headers a consumer
reads, and on the body. Bodies are compared as JSON when both parse as JSON,
otherwise as text; in both cases timestamps are replaced by `<ts>`, because the
reference writes `+00:00` and omits a zero fraction where this port writes six
digits and `Z` (a recorded difference — one timestamp spelling, one meaning).
"""

from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from typing import Any

import httpx

TOKEN = "tok-spec-review"
API_KEY = {"X-API-Key": TOKEN}

# The headers a consumer can act on. `content-length` is left out: it follows
# the body, which is compared separately.
COMPARED_HEADERS = (
    "content-type",
    "location",
    "allow",
    "cache-control",
    "www-authenticate",
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-allow-credentials",
    "access-control-max-age",
    "vary",
)

# Every difference this battery may find, as a `differences.md` row. The value
# is what the record says, so the report can say which row "known" means.
KNOWN: dict[str, str] = {
    "K-timestamp": "Timestamps: the reference writes `+00:00` and omits a zero fraction; this port writes six digits and `Z`. Normalised, not reported.",
    "K-report-error": "Report `error` text: `ExceptionType: message` in the reference, a Rust error description here, same 2000-character cap (differences.md 06).",
    "K-parse-suffix": "JUnit parse-error detail suffix: ElementTree/expat wording vs roxmltree wording; the prefix and the 422 stay (differences.md 02).",
    "K-path-id-float": "A float path id: the reference coerces `1.0` to 1, this port answers 422 `int_parsing` (differences.md 20, S12).",
    "K-path-id-range": "A path id past `int4`: the reference answers 500, this port answers 404 (differences.md 20, S12).",
    "K-nul": "A NUL in request text: the reference answers 500, this port answers 422 `value_error` (differences.md 20, H19a).",
    "K-page-overflow": "A `page` whose OFFSET does not fit an i64: the reference answers 500 or wraps, this port answers 422 `less_than_equal` (differences.md 20, A4/H2).",
    "K-nonfinite-time": "A JUnit `time` that is not finite: the reference stores it and serves JSON null, this port answers 400 (differences.md 19, H19b).",
    "K-attempt-past-int4": "A `ci_run_attempt` past `int4`: the reference answers 500, this port answers 400 (differences.md 06).",
    "K-head-options": "`HEAD`/`OPTIONS` on a route that has neither: the reference answers 405, this port answers 200 with an `Allow` header (differences.md 20, S12).",
    "K-cors-preflight": "A preflight from a disallowed origin: the reference answers 400 `Disallowed CORS origin` with no `access-control-max-age`, this port answers 200 (differences.md 20, S12).",
    "K-api-slash": "`GET /api/`: the reference answers 404, this port 307 to `/api` (differences.md 20).",
    "K-multipart-detail": "The detail of a multipart body the reference's form parser refuses, and of a part with no `name`: the reference answers Starlette's or `python_multipart`'s own text, this port answers `Empty report body` (differences.md 20, S12).",
    "K-newline-line": "The `line` attribute: the reference accepts Unicode digits, this port accepts ASCII digits only (differences.md 04).",
    "K-mcp-envelope": "The MCP result envelope: fastmcp writes explicit nulls, its own version and the deprecated `logging` capability; rmcp omits them and adds `resultType`/`ttlMs`/`cacheScope` (differences.md 11).",
    "K-mcp-error-text": "The text of an MCP failure result: SQLAlchemy/Pydantic taxonomy in the reference, serde/SeaORM wording here; the status, envelope, `isError` and field line match (differences.md 11).",
    "K-mcp-bad-message": "A `POST /mcp/` body that is JSON but not a JSON-RPC message: the reference answers 400 `-32602` with a Pydantic union report, this port answers 415 (differences.md 11).",
    "K-mcp-session": "MCP sessions: the reference keeps a session id, this port is stateless, so `GET`/`DELETE` on `/mcp/` answer 400 in the reference and 405 here, and a tokenless `PUT` is 401 here (differences.md 01).",
    "K-openapi-doc": "The `/openapi.json` document: fastapi's prose, operation ids and schema spelling vs utoipa's (differences.md 14). Task 22 compared the paths, methods, parameters and schema types.",
    "K-transient-retry": "A failure the reference's own docstring calls transient (SQLSTATE 40001) fails the Report at once there, because SQLAlchemy does not wrap asyncpg's SerializationError as OperationalError; this port retries it up to max_report_retries (differences.md 23).",
    "K-body-limit": "A request body past 20 MiB on a JSON route: this port answers 413 with its own detail, the reference read the body and validated it (differences.md 23).",
    "K-framework-422-fields": "The `input` and `ctx` of a framework-generated 422 (and the number of missing query parameters it lists): the type, loc, msg and shape are identical, and no consumer reads these (00-shared-context.md, the 'May differ' row).",
    "K-json-invalid-offset": "The offset in a `json_invalid` 422's loc and its `ctx.error` sentence: serde's error position against Python's json module (differences.md 23).",
    "K-delete-204-content-type": "A 204 answer: the reference labels it `application/json` with no body, this port sends no content type (differences.md 23).",
    "K-racing-delete-counts": "The `deleted` counts of a Repo or Project delete that races the processor: both servers read the counts in the deleting transaction, so a Report that commits between the count and the delete is deleted but not counted, and the two answers can differ on the same race. The rows left behind agree, and are compared (differences.md 23).",
    "K-metrics-text": "The `/metrics` text: the reference and this port both serve the recorded series; compared as text, where a counter's own value is normalised.",
}

TS = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?")
DATE = re.compile(r"\b\d{4}-\d\d-\d\d\b")


def normalize(value: Any) -> Any:
    """Timestamps replaced, so two clocks never differ; everything else kept."""
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, str):
        return TS.sub("<ts>", value)
    return value


def _mask(value: Any, masked: list[str], path: str = "") -> Any:
    """`mask` names dotted keys whose value may differ, `<masked>` in their place."""
    if isinstance(value, dict):
        return {
            k: ("<masked>" if f"{path}.{k}".lstrip(".") in masked else _mask(v, masked, f"{path}.{k}"))
            for k, v in value.items()
        }
    if isinstance(value, list):
        return [_mask(v, masked, path) for v in value]
    return value


@dataclass
class Reply:
    status: int
    headers: dict[str, str]
    text: str
    json: Any = None

    @property
    def is_json(self) -> bool:
        return self.json is not None


def _read(response: httpx.Response) -> Reply:
    text = response.text
    try:
        parsed = response.json()
    except ValueError:
        parsed = None
    headers = {h: response.headers[h].lower() for h in COMPARED_HEADERS if h in response.headers}
    if "location" in headers:
        # A redirect target names the recorder's own port; compare the path.
        headers["location"] = re.sub(r"^http://127\.0\.0\.1:\d+", "", headers["location"])
    return Reply(status=response.status_code, headers=headers, text=text, json=parsed)


@dataclass
class Difference:
    phase: str
    case: str
    known: str | None
    python: Reply
    rust: Reply
    reason: str

    def render(self) -> str:
        def one(reply: Reply) -> str:
            body = reply.text if reply.json is None else json.dumps(normalize(reply.json), sort_keys=True, ensure_ascii=False)
            return f"{reply.status} {reply.headers} {body[:700]}"

        return (
            f"[{self.phase}] {self.case}\n"
            f"  reason: {self.reason}\n"
            f"  python: {one(self.python)}\n"
            f"  rust:   {one(self.rust)}"
        )


@dataclass
class Battery:
    py: str
    rs: str
    pg_container: str
    phase: str = ""
    cases: int = 0
    differences: list[Difference] = field(default_factory=list)
    known_counts: dict[str, int] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.pc = httpx.Client(base_url=self.py, timeout=120, follow_redirects=False)
        self.rc = httpx.Client(base_url=self.rs, timeout=120, follow_redirects=False)

    # ── requests ──────────────────────────────────────────────────────────

    def call(
        self,
        method: str,
        path: str,
        *,
        params: list[tuple[str, str]] | None = None,
        query: str | None = None,
        headers: dict[str, str] | None = None,
        content: bytes | None = None,
        json_body: Any = None,
        known: str | None = None,
        mask: tuple[str, ...] = (),
        label: str | None = None,
    ) -> tuple[Reply, Reply]:
        """Send one request to both servers and compare the two answers."""
        url = path if query is None else f"{path}?{query}"
        self.cases += 1
        case = label or f"{method} {url}" + (f" {json_body!r}"[:120] if json_body is not None else "")
        assert known is None or known in KNOWN, known

        def send(client: httpx.Client) -> Reply:
            try:
                response = client.request(
                    method, url, params=params, headers=headers, content=content, json=json_body
                )
            except httpx.HTTPError as error:
                # The reference aborts some hostile requests (a `ci_run_attempt`
                # past the column, for one). A dropped connection is its answer,
                # so it is recorded rather than raised: the status 0 says the
                # server closed the request instead of answering it.
                return Reply(status=0, headers={}, text=f"<transport error: {type(error).__name__}>")
            return _read(response)

        py, rs = send(self.pc), send(self.rc)
        reason = self._reason(case, py, rs, masked=mask)
        if reason is None:
            return py, rs
        difference = Difference(self.phase, case, known, py, rs, reason)
        if known is None:
            self.differences.append(difference)
        else:
            self.known_counts[known] = self.known_counts.get(known, 0) + 1
        return py, rs

    def _reason(self, case: str, py: Reply, rs: Reply, masked: tuple[str, ...]) -> str | None:
        if py.status != rs.status:
            return f"status {py.status} vs {rs.status}"
        if py.headers != rs.headers:
            keys = sorted(set(py.headers) | set(rs.headers))
            changed = [k for k in keys if py.headers.get(k) != rs.headers.get(k)]
            return f"header(s) {', '.join(changed)}"
        if py.is_json and rs.is_json:
            left = _mask(normalize(py.json), list(masked))
            right = _mask(normalize(rs.json), list(masked))
            if left != right:
                return f"body {json.dumps(left, ensure_ascii=False)[:400]} vs {json.dumps(right, ensure_ascii=False)[:400]}"
            return None
        left, right = normalize(py.text), normalize(rs.text)
        if left != right:
            return f"text {left[:300]!r} vs {right[:300]!r}"
        return None

    def note(self, text: str) -> None:
        self.notes.append(f"[{self.phase}] {text}")

    def compare(
        self, label: str, python: Any, rust: Any, *, known: str | None = None, mask: tuple[str, ...] = ()
    ) -> None:
        """Compare two values a caller already gathered (an MCP answer, a webhook body)."""
        self.cases += 1
        assert known is None or known in KNOWN, known
        left = Reply(200, {}, json.dumps(python, sort_keys=True), json=python)
        right = Reply(200, {}, json.dumps(rust, sort_keys=True), json=rust)
        reason = self._reason(label, left, right, masked=mask)
        if reason is None:
            return
        difference = Difference(self.phase, label, known, left, right, reason)
        if known is None:
            self.differences.append(difference)
        else:
            self.known_counts[known] = self.known_counts.get(known, 0) + 1

    # ── rows ──────────────────────────────────────────────────────────────

    def psql(self, database: str, sql: str) -> str:
        result = subprocess.run(
            ["docker", "exec", self.pg_container, "psql", "-U", "fr", "-d", database, "-Atc", sql],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(f"psql {database}: {result.stderr.strip()}")
        return result.stdout

    def rows(self, sql: str, *, label: str | None = None, known: str | None = None, mask: tuple[str, ...] = ()) -> bool:
        """Compare one query's rows on both databases. Returns whether they agree."""
        self.cases += 1
        case = label or f"rows: {sql}"
        py, rs = self.psql("py", sql), self.psql("rs", sql)
        if py == rs:
            return True
        reason = f"rows differ: python {py[:600]!r} vs rust {rs[:600]!r}"
        if known is None:
            self.differences.append(
                Difference(self.phase, case, None, Reply(0, {}, py), Reply(0, {}, rs), reason)
            )
        else:
            self.known_counts[known] = self.known_counts.get(known, 0) + 1
        return False

    def dump(self, tables: list[str | tuple[str, str]], *, label: str) -> bool:
        """Compare every row of the named tables, as JSON, timestamps normalised.

        An entry is a table name (ordered by `id`) or `(table, order_by)` for a
        table whose rows have no `id`, such as the Score history tables.
        """
        entries = [entry if isinstance(entry, tuple) else (entry, "id") for entry in tables]
        parts = [
            f"'{table}', (SELECT coalesce(json_agg(t), '[]'::json) FROM (SELECT * FROM {table} ORDER BY {order}) t)"
            for table, order in entries
        ]
        sql = "SELECT json_build_object(" + ", ".join(parts) + ")"
        self.cases += 1
        py = normalize(json.loads(self.psql("py", sql)))
        rs = normalize(json.loads(self.psql("rs", sql)))
        if py == rs:
            return True
        # Point at the first table and row that differ, so the report is usable.
        where = "document"
        if isinstance(py, dict):
            for table in py:
                if py[table] != rs.get(table):
                    where = table
                    break
        self.differences.append(
            Difference(
                self.phase,
                label,
                None,
                Reply(0, {}, json.dumps(py if not isinstance(py, dict) else py.get(where), sort_keys=True)[:1500]),
                Reply(0, {}, json.dumps(rs if not isinstance(rs, dict) else rs.get(where), sort_keys=True)[:1500]),
                f"rows differ in {where}",
            )
        )
        return False

    # ── waiting ───────────────────────────────────────────────────────────

    def drain(self, *, timeout: float = 120.0) -> None:
        """Wait until both servers have processed every queued Report."""
        import time

        deadline = time.monotonic() + timeout
        while True:
            pending = []
            for client in (self.pc, self.rc):
                pending.append(client.get("/api/reports/summary").json()["pending"])
            if pending == [0, 0]:
                return
            if time.monotonic() > deadline:
                raise RuntimeError(f"reports still pending after {timeout}s: {pending}")
            time.sleep(0.02)

    def idle(self) -> None:
        """Wait until both servers answer their own health probe."""
        import time

        for _ in range(600):
            ok = []
            for client in (self.pc, self.rc):
                try:
                    ok.append(client.get("/api/health/ready").status_code == 200)
                except httpx.HTTPError:
                    ok.append(False)
            if all(ok):
                return
            time.sleep(0.1)
        raise RuntimeError("a server never became ready")

    # ── reporting ─────────────────────────────────────────────────────────

    def summary(self) -> dict[str, Any]:
        return {
            "phase": self.phase,
            "cases": self.cases,
            "newDifferences": len(self.differences),
            "knownDifferences": dict(sorted(self.known_counts.items())),
            "notes": self.notes,
        }
