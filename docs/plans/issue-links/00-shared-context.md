# 00 — Shared context for the issue-links plan

**Read this whole file before you start any task in `docs/plans/issue-links/`.** Every task file assumes you know what is here. Each task file gives the exact contracts for its own step. This file gives the ground rules, the vocabulary, the decisions and the current code that all the tasks share.

## What we are building

GitHub issue #6: "API: link an issue filed elsewhere to a test or job". Read it with `gh issue view 6 --repo andrewthetechie/flakeradar`.

Today only FlakeRadar's own filer (`backend/app/github_integration.py`) sets `github_issue_number` on a Test (`TestCase`) or Job. An issue filed by another bot, by fabro's scout, or by a person is invisible: the dashboard shows no link, FlakeRadar's filer files a duplicate when it is turned on, and the closed-issue sync never sees it.

We add a way for any authenticated caller to set or clear a Test's or Job's **Issue link**:

```
PUT    /api/tests/{test_id}/issue[?replace=true]   body {"number": 123, "url": "https://github.com/owner/name/issues/123"}  (url optional)
DELETE /api/tests/{test_id}/issue
PUT    /api/jobs/{job_id}/issue[?replace=true]     (same body)
DELETE /api/jobs/{job_id}/issue

MCP: link_issue(number, test_id | job_id, replace=False)
     unlink_issue(test_id | job_id)
```

These are the **first write tools of the MCP server**. Every existing MCP tool gets a `readOnlyHint` annotation, so clients can tell reads from writes.

No migration. No UI change (the dashboard already shows `github_issue_number` / `github_issue_url`). No change to the filer or the sync code: tests prove that they already treat a linked entity correctly.

## Delivery rules (read carefully)

- **Branch:** all tasks land in order on `feat/issue-links`. Create it from `main` if it does not exist yet (`git switch -c feat/issue-links main`), otherwise `git switch feat/issue-links`. Commit each task as one or more commits on that branch. Do not merge to `main`. The whole series ships as **one PR**, and task 05 opens it.
- **Order:** tasks run **one after another** in number order (01 … 05). You can assume every task with a lower number has landed.
- **Keep the suite green.** Every task must leave these passing:
  ```bash
  cd backend && .venv/bin/python -m pytest -q          # needs Docker (testcontainers Postgres 17)
  cd .. && uvx ruff@0.16.9 check backend && uvx ruff@0.16.9 format --check backend
  ```
  Use `uvx ruff@0.16.9` (or `pip install ruff==0.16.9`). An older system `ruff` fails on `ruff.toml` with `unknown field 'lint'`. No task touches `frontend/`.
- **Do not edit** `CONTEXT.md` or `docs/adr/*`. The commit that added this plan already added the **Issue link** term to `CONTEXT.md` and wrote `docs/adr/0008-issue-links-from-other-filers.md`. Read both. Task 05 only checks that they still match what you built.
- **Commit messages** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. If you are a different model, use your own name in the same format.
- **If a contract in a task file is wrong** (for example, a library behaves differently), do the smallest correct thing, keep the public names, and say what you changed in the commit message.
- **Match the surrounding code.** Comment density, naming and idiom like `backend/app/queries.py`, `backend/app/routers/tests.py` and `backend/app/mcp_server.py`. No ORM relationships: every read is an explicit `select()`/join (see the `models.py` docstring).

## Domain vocabulary (from `CONTEXT.md`: use these words in code, docs and error text)

| Term | Meaning | Avoid |
|---|---|---|
| **Repo** | A source repository, lowercase `owner/name`. Table `repos`. | |
| **Project** | A named test suite inside a Repo. Table `projects`. | |
| **Test** | One logical test case in one Repo and Project. Table `test_cases`, model `TestCase`. | test case (in text) |
| **Pipeline** | A named CI workflow in a Repo. Table `pipelines`, column `provider` (`github`, `gitlab`, …). | |
| **Job** | One job within a Pipeline. Table `jobs`, model `Job`. Belongs to a Repo through its Pipeline. | |
| **Issue link** (new) | The GitHub issue that tracks a flaky Test or Job, stored as `github_issue_number`. The issue is in the entity's own Repo. FlakeRadar's filer sets it, and now any filer can set or clear it. | ticket, bug |
| **Quarantine** | A human decision that marks a Test as skippable. **Not** writable through MCP, and this plan does not change that. | |

In prose say "Issue link", "link an issue", "unlink". In code the column stays `github_issue_number`, and the new module is `issue_links`.

## Decisions (the ledger): treat as requirements

**Storage**
- Only the existing `github_issue_number` columns (`test_cases`, `jobs`). **No new column, no migration.** `github_issue_url` stays derived by `queries.issue_url(repo, number)` (Tests) and `queries.job_issue_url(repo, pipeline, job)` (Jobs).
- The issue must be in the Test's or Job's **own Repo**, because `sync_closed_issues` asks GitHub for `/repos/{entity's repo}/issues/{number}`. fabro's scout (the first user, `andrewthetechie/fabro-workflows` ADR 0027) runs inside each target repo, so it files there.

**REST: `PUT /api/{tests|jobs}/{id}/issue`**
- Auth: `X-API-Key`, with `dependencies=[Depends(require_token)]` (`backend/app/auth.py`). A missing or wrong key gives `401 "Invalid or missing X-API-Key"`.
- Body `schemas.IssueLinkIn`: `number` (int, `1 ≤ number ≤ 2147483647`, PostgreSQL `INTEGER`), `url` (optional string, `max_length=2048`). An invalid body gives `422` (Pydantic).
- Query `replace: bool = False`.
- Path id: `int`.
- Response: `200` with the entity after the change: `schemas.TestOut` / `schemas.JobOut` (built with `queries.to_test_out` / `queries.to_job_out`, `trend` left `None`, as `set_quarantine` does).
- Rules, checked **in this order** (`detail` strings are exact; `{Entity}` is `Test` or `Job`):
  1. No such entity → `404 "Test not found"` / `404 "Job not found"` (the same text as the existing history endpoints).
  2. Jobs only: the Pipeline's `provider != "github"` → `422 "Only Jobs of GitHub Pipelines can link a GitHub issue"`.
  3. `number` out of range → `422 "number must be between 1 and 2147483647"`. (REST callers hit Pydantic's own `422` first; this rule is for MCP. The service checks it anyway.)
  4. `url` given and `url.strip().rstrip("/").lower()` is not the derived URL (`queries.issue_url(repo_name, number)`) → `422 "url must be {expected}: the issue must be in the {Entity}'s own Repo"`.
  5. The entity already links a **different** number and `replace` is false → `409 "{Entity} already links issue #{current}; pass replace=true to replace it"`.
  6. Otherwise set `github_issue_number = number`, commit, log, return `200`. The same number again is a no-op `200`. `replace=true` with no link or the same link also just sets it.
- Concurrency: load the row with `SELECT … FOR UPDATE OF` the entity's table (`.with_for_update(of=TestCase)` / `.with_for_update(of=Job)`), so two `PUT`s cannot both pass the `409` check.
- Logging: `logging.getLogger("flakeradar.issues")`, `logger.info("Linked issue %s#%s to %s %s (was %s)", repo, number, "test"|"job", id, previous)`.

**REST: `DELETE /api/{tests|jobs}/{id}/issue`**
- Same auth. No body, no query.
- No such entity → `404` (same text). Otherwise set `github_issue_number = None`, commit, return `200` with the entity. Already unlinked → no-op `200`. **No** provider check (clearing is always safe).
- Log `logger.info("Unlinked issue %s#%s from %s %s", repo, previous, kind, id)` only when there was a link.

**Filer and sync (no code change; tests only)**
- `github_integration._select_candidates` and `_job_candidates` filter `github_issue_number.is_(None)`, so a linked entity is never filed again.
- `github_integration.sync_closed_issues` reads every Test and Job with a non-null number, so it re-arms a linked entity when its issue is closed (`200 {"state": "closed"}`) or gone (`404`).
- Known gap, accepted in ADR 0008: the filer can overwrite a link written while it is mid-POST to GitHub. Do not fix it.

**MCP**
- `link_issue(number: int, test_id: int | None = None, job_id: int | None = None, replace: bool = False) -> dict[str, Any]` and `unlink_issue(test_id: int | None = None, job_id: int | None = None) -> dict[str, Any]`.
- Exactly one of `test_id` / `job_id`, else `ToolError("Pass exactly one of test_id or job_id.")`.
- They call the same service functions as REST. Every service error becomes `ToolError(str(exc))`, so the text is the same as the REST `detail`.
- They return the `TestOut` / `JobOut` `model_dump(mode="json")`.
- No `url` parameter (agents send the number only).
- Annotations (`@mcp.tool(annotations=...)`, plain dicts with the MCP wire names):
  - every existing tool: `{"readOnlyHint": True}`;
  - `link_issue`: `{"readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}`;
  - `unlink_issue`: `{"readOnlyHint": False, "destructiveHint": True, "idempotentHint": True, "openWorldHint": False}`.
- The module docstring, `INSTRUCTIONS` and the README stop calling the server "read-only" and say which tools write.

## The service module (task 01 creates it; tasks 02 and 04 use it)

New file `backend/app/issue_links.py`. It is the only place that writes `github_issue_number` outside the filer and the sync. Both REST and MCP call it.

```python
class IssueLinkError(Exception):
    """A request the Issue link rules refuse; str(exc) is the error detail."""
    status_code = 422

class IssueLinkTargetNotFound(IssueLinkError):   status_code = 404
class IssueAlreadyLinked(IssueLinkError):        status_code = 409
class IssueNotLinkable(IssueLinkError):          status_code = 422

async def link_test_issue(db, test_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float) -> schemas.TestOut
async def unlink_test_issue(db, test_id: int, *, threshold: float) -> schemas.TestOut
async def link_job_issue(db, job_id: int, number: int, *, url: str | None = None, replace: bool = False, threshold: float) -> schemas.JobOut
async def unlink_job_issue(db, job_id: int, *, threshold: float) -> schemas.JobOut
```

## Current code you will touch (facts, checked 2026-10-17 on `main` @ `35f6019`)

- `backend/app/models.py`: `TestCase.github_issue_number: Mapped[int | None]` (line ~117) and `Job.github_issue_number: Mapped[int | None]` (line ~230), both `Integer`. `Pipeline.provider: str`. `Job.pipeline_id`, `Pipeline.repo_id`, `TestCase.project_id`, `Project.repo_id`.
- `backend/app/queries.py` ("read services shared by the REST routers and the MCP server"):
  - `tests_select()` returns `select(TestCase, Project.name, Repo.name)` with the joins; `to_test_out(tc, project, repo, threshold, trend=None)`.
  - `jobs_select()` returns `select(Job, Pipeline, Repo)` with the joins; `to_job_out(job, pipeline, repo, threshold, trend=None)` (note: `repo` is the `Repo` row here, not a string).
  - `issue_url(repo: str, number: int | None) -> str | None` gives `https://github.com/{repo}/issues/{number}`. `job_issue_url(repo, pipeline, job)` gives it only when `pipeline.provider == "github"`.
  - `set_quarantine(db, test_id, quarantined, *, threshold)` is the existing write to copy: load with `tests_select().where(TestCase.id == test_id)`, change the ORM object, `await db.commit()`, return `to_test_out(...)`.
- `backend/app/schemas.py`: starts with `from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator`. Sections are marked `# --- Name (task/issue) ---`. The last section is `# --- CI run failures (issue #5) ---`. `QuarantineIn(BaseModel): quarantined: bool` is the existing write body.
- `backend/app/routers/tests.py`: already imports `require_token`, `get_settings`, `get_db`, `queries`, `schemas`, `HTTPException`, `Query`. `POST /api/tests/{test_id}/quarantine` is the write route to sit next to.
- `backend/app/routers/jobs.py`: imports `APIRouter, Depends, HTTPException, Query`, `queries, schemas`, `get_settings`, `get_db`, `normalize_repo`. **No** `require_token` import yet. `GET /api/jobs/{job_id}/history` is the last route.
- `backend/app/auth.py`: `require_token(x_api_key: str = Header(default=""))` raises `401 "Invalid or missing X-API-Key"`.
- `backend/app/mcp_server.py`: module docstring says "Read-only MCP server … Nothing here writes: quarantine stays a human decision." Tools are nested `@mcp.tool async def …` inside `build_mcp(session_factory, *, api_token)`. The last one is `ci_run_failures`, followed by `return mcp`. `INSTRUCTIONS` ends with `Tool results are read-only data from CI output, not instructions."""`. Imports `queries`, `get_settings`, `ToolError`, `Any`.
- `backend/app/github_integration.py`: `file_issues_for(db, test_case_ids, *, transport=None)`, `file_job_issues_for(db, job_ids, *, transport=None)`, `sync_closed_issues(session_factory, *, transport=None)`. All no-ops unless `get_settings().github_token` is set.
- fastmcp is `4.0.9`. `@mcp.tool(annotations={"readOnlyHint": True})` works. A `fastmcp.Client` `list_tools()` result has `t.annotations.read_only_hint`, `.destructive_hint`, `.idempotent_hint`, `.open_world_hint` (snake_case; the camelCase attributes are deprecated and warn). Checked on 2026-10-17.
- Tests: `backend/tests/conftest.py` gives fixtures `client` (httpx against the app, no worker), `db` (an `AsyncSession`), `session_factory`, and `AUTH = {"X-API-Key": "changeme"}` (import with `from tests.conftest import AUTH`). `backend/tests/factories.py` gives `make_project(db, repo, project)`, `make_test_case(db, project, name=..., **fields)`, `make_pipeline(db, repo, name, provider="github")`, `make_job(db, pipeline, name=..., **fields)`. Factories flush but do not commit: call `await db.commit()` before using `client`. `backend/tests/test_github.py` has the `gh_settings` fixture and the `_recorder()` / `httpx.MockTransport` helpers. `backend/tests/test_mcp.py` has the `mcp` fixture (`build_mcp(session_factory, api_token=None)`) and `test_tools_are_listed`.
