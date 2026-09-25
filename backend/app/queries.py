"""Read services shared by the REST routers and the MCP server.

Every function takes an AsyncSession and returns schemas.* models, so both
front doors (HTTP and MCP) expose exactly the same data.
"""

from dataclasses import dataclass
from typing import Literal

from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from . import schemas
from .models import Project, Repo, TestCase, TestExecution, TestRun, utcnow

SortKey = Literal["score", "last_seen", "proven"]


@dataclass(frozen=True)
class Scope:
    """Which Tests a query covers. `project` requires `repo` (names overlap across Repos)."""

    repo: str | None = None
    project: str | None = None

    def __post_init__(self) -> None:
        if self.project is not None and self.repo is None:
            raise ValueError("project filter requires repo")


def tier_for(score: float, threshold: float) -> schemas.Tier:
    if score >= threshold:
        return "flaky"
    if score > 0:
        return "suspect"
    return "stable"


def escape_like(text: str) -> str:
    """Escape LIKE wildcards so user text matches literally (used with escape='\\\\')."""
    return text.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def to_test_out(tc: TestCase, project: str, repo: str, threshold: float) -> schemas.TestOut:
    return schemas.TestOut(
        id=tc.id,
        repo=repo,
        project=project,
        fingerprint=tc.fingerprint,
        suite=tc.suite,
        classname=tc.classname,
        name=tc.name,
        file=tc.file,
        line=tc.line,
        flakiness_score=tc.flakiness_score,
        tier=tier_for(tc.flakiness_score, threshold),
        confirmed_flake_count=tc.confirmed_flake_count,
        last_status=tc.last_status,
        last_seen_at=tc.last_seen_at,
        quarantined=tc.quarantined,
        quarantined_at=tc.quarantined_at,
        github_issue_number=tc.github_issue_number,
    )


def tests_select() -> Select:
    """SELECT TestCase, Project.name, Repo.name — the row shape to_test_out() takes."""
    return (
        select(TestCase, Project.name, Repo.name)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
    )


def _scoped(stmt: Select, scope: Scope) -> Select:
    """Filter a statement that already joins Project and Repo."""
    if scope.repo is not None:
        stmt = stmt.where(Repo.name == scope.repo)
    if scope.project is not None:
        stmt = stmt.where(Project.name == scope.project)
    return stmt


async def list_repos(db: AsyncSession) -> list[schemas.RepoOut]:
    rows = (
        await db.execute(
            select(Repo.name, Project.name, Project.root)
            .join(Project, Project.repo_id == Repo.id)
            .order_by(Repo.name, Project.name)
        )
    ).all()
    repos: dict[str, schemas.RepoOut] = {}
    for repo, project, root in rows:
        repos.setdefault(repo, schemas.RepoOut(name=repo, projects=[]))
        repos[repo].projects.append(schemas.ProjectOut(name=project, root=root))
    return list(repos.values())


async def list_tests(
    db: AsyncSession,
    scope: Scope,
    *,
    threshold: float,
    include_stable: bool = False,
    flaky_only: bool = False,
    sort: SortKey = "score",
    page: int = 1,
    page_size: int = 50,
    file: str | None = None,
) -> schemas.TestPage:
    stmt = _scoped(tests_select(), scope)
    if flaky_only:
        stmt = stmt.where(TestCase.flakiness_score >= threshold)
    elif not include_stable:
        stmt = stmt.where(TestCase.flakiness_score > 0)
    if file:
        stmt = stmt.where(TestCase.file.ilike(f"%{escape_like(file)}%", escape="\\"))

    total = (await db.execute(select(func.count()).select_from(stmt.subquery()))).scalar_one()

    order = {
        "score": (TestCase.flakiness_score.desc(), TestCase.last_seen_at.desc(), TestCase.id),
        "last_seen": (TestCase.last_seen_at.desc(), TestCase.id.desc()),
        "proven": (TestCase.confirmed_flake_count.desc(), TestCase.flakiness_score.desc(), TestCase.id),
    }[sort]
    rows = (await db.execute(stmt.order_by(*order).offset((page - 1) * page_size).limit(page_size))).all()
    return schemas.TestPage(
        items=[to_test_out(tc, project, repo, threshold) for tc, project, repo in rows],
        total=total,
        page=page,
        page_size=page_size,
    )


async def summary(db: AsyncSession, scope: Scope, *, threshold: float) -> schemas.SummaryOut:
    def tests_count(*conditions) -> Select:
        return _scoped(
            select(func.count(TestCase.id))
            .join(Project, TestCase.project_id == Project.id)
            .join(Repo, Project.repo_id == Repo.id)
            .where(*conditions),
            scope,
        )

    total = (await db.execute(tests_count())).scalar_one()
    flaky = (await db.execute(tests_count(TestCase.flakiness_score >= threshold))).scalar_one()
    suspect = (
        await db.execute(tests_count(TestCase.flakiness_score > 0, TestCase.flakiness_score < threshold))
    ).scalar_one()
    confirmed = (await db.execute(tests_count(TestCase.confirmed_flake_count > 0))).scalar_one()
    runs = (
        await db.execute(
            _scoped(
                select(func.count(TestRun.id))
                .join(Project, TestRun.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id),
                scope,
            )
        )
    ).scalar_one()
    executions = (
        await db.execute(
            _scoped(
                select(func.count(TestExecution.id))
                .join(TestCase, TestExecution.test_case_id == TestCase.id)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id),
                scope,
            )
        )
    ).scalar_one()
    return schemas.SummaryOut(
        total_tests=total,
        flaky_tests=flaky,
        suspect_tests=suspect,
        confirmed_flaky_tests=confirmed,
        total_runs=runs,
        total_executions=executions,
        flake_threshold=threshold,
    )


# --- Test detail and quarantine (task 09) -------------------------------

FAILING = ("failed", "error")


def repo_path(root: str, file: str) -> str:
    """A reported file path made relative to the Repo root."""
    return f"{root}/{file}" if root else file


def permalink(repo: str, sha: str, root: str, file: str, line: int | None) -> str:
    url = f"https://github.com/{repo}/blob/{sha}/{repo_path(root, file)}"
    return f"{url}#L{line}" if line is not None and line >= 1 else url


async def get_test(
    db: AsyncSession, test_id: int, *, threshold: float, executions_limit: int = 60
) -> schemas.HistoryOut | None:
    row = (await db.execute(tests_select().add_columns(Project.root).where(TestCase.id == test_id))).first()
    if row is None:
        return None
    tc, project, repo, root = row

    rows = (
        await db.execute(
            select(TestExecution, TestRun)
            .join(TestRun, TestExecution.test_run_id == TestRun.id)
            .where(TestExecution.test_case_id == test_id)
            .order_by(TestExecution.id.desc())
            .limit(executions_limit)
        )
    ).all()
    executions = [
        schemas.ExecutionOut(
            id=e.id,
            status=e.status,
            duration=e.duration,
            message=e.message,
            details=e.details,
            created_at=e.created_at,
            commit_sha=r.commit_sha,
            branch=r.branch,
            ci_run_id=r.ci_run_id,
        )
        for e, r in rows
    ]

    failing = (
        await db.execute(
            select(TestRun.commit_sha, TestRun.branch)
            .join(TestExecution, TestExecution.test_run_id == TestRun.id)
            .where(TestExecution.test_case_id == test_id, TestExecution.status.in_(FAILING))
            .order_by(TestExecution.id.desc())
            .limit(1)
        )
    ).first()
    last_sha, last_branch = failing if failing is not None else (None, None)

    location = None
    if tc.file:
        location = schemas.LocationOut(
            path=repo_path(root, tc.file),
            line=tc.line,
            url=permalink(repo, last_sha, root, tc.file, tc.line) if last_sha else None,
        )
    return schemas.HistoryOut(
        test=to_test_out(tc, project, repo, threshold),
        location=location,
        last_failing_sha=last_sha,
        last_failing_branch=last_branch,
        executions=executions,
    )


async def set_quarantine(
    db: AsyncSession, test_id: int, quarantined: bool, *, threshold: float
) -> schemas.TestOut | None:
    row = (await db.execute(tests_select().where(TestCase.id == test_id))).first()
    if row is None:
        return None
    tc, project, repo = row
    tc.quarantined = quarantined
    tc.quarantined_at = utcnow() if quarantined else None
    await db.commit()
    return to_test_out(tc, project, repo, threshold)


async def quarantine_list(db: AsyncSession, repo: str, project: str) -> list[schemas.QuarantineItem]:
    rows = (
        (
            await db.execute(
                select(TestCase)
                .join(Project, TestCase.project_id == Project.id)
                .join(Repo, Project.repo_id == Repo.id)
                .where(Repo.name == repo, Project.name == project, TestCase.quarantined.is_(True))
                .order_by(TestCase.name, TestCase.id)
            )
        )
        .scalars()
        .all()
    )
    return [
        schemas.QuarantineItem(
            suite=tc.suite,
            classname=tc.classname,
            name=tc.name,
            fingerprint=tc.fingerprint,
            file=tc.file,
            line=tc.line,
            quarantined_at=tc.quarantined_at,
        )
        for tc in rows
    ]


# --- Search and name lookup (task 10, used by the MCP server) ----------


async def search_tests(
    db: AsyncSession, scope: Scope, query: str, *, threshold: float, limit: int = 20
) -> list[schemas.TestOut]:
    """Case-insensitive literal substring over name, classname and file; worst first."""
    pattern = f"%{escape_like(query)}%"
    stmt = _scoped(tests_select(), scope).where(
        TestCase.name.ilike(pattern, escape="\\")
        | TestCase.classname.ilike(pattern, escape="\\")
        | TestCase.file.ilike(pattern, escape="\\")
    )
    rows = (await db.execute(stmt.order_by(TestCase.flakiness_score.desc(), TestCase.id).limit(limit))).all()
    return [to_test_out(tc, project, repo, threshold) for tc, project, repo in rows]


async def find_test_ids(
    db: AsyncSession, repo: str, project: str, name: str, classname: str | None = None
) -> list[int]:
    """Exact-match lookup of a Test by name (and optionally classname)."""
    stmt = _scoped(
        select(TestCase.id)
        .join(Project, TestCase.project_id == Project.id)
        .join(Repo, Project.repo_id == Repo.id)
        .where(TestCase.name == name),
        Scope(repo=repo, project=project),
    )
    if classname is not None:
        stmt = stmt.where(TestCase.classname == classname)
    return list((await db.execute(stmt.order_by(TestCase.id))).scalars())


async def latest_failure(db: AsyncSession, test_id: int) -> schemas.ExecutionOut | None:
    row = (
        await db.execute(
            select(TestExecution, TestRun)
            .join(TestRun, TestExecution.test_run_id == TestRun.id)
            .where(TestExecution.test_case_id == test_id, TestExecution.status.in_(FAILING))
            .order_by(TestExecution.id.desc())
            .limit(1)
        )
    ).first()
    if row is None:
        return None
    e, r = row
    return schemas.ExecutionOut(
        id=e.id,
        status=e.status,
        duration=e.duration,
        message=e.message,
        details=e.details,
        created_at=e.created_at,
        commit_sha=r.commit_sha,
        branch=r.branch,
        ci_run_id=r.ci_run_id,
    )
