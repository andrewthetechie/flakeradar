"""Read services shared by the REST routers and the MCP server.

Every function takes an AsyncSession and returns schemas.* models, so both
front doors (HTTP and MCP) expose exactly the same data.
"""

from collections import defaultdict
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from datetime import timedelta
from typing import Any, Literal

from sqlalchemy import ColumnElement, Select, func, or_, select, true
from sqlalchemy.ext.asyncio import AsyncSession

from . import schemas
from .attribution import (
    explained_ci_job_ids,
    job_execution_explained,
    job_execution_window_ids,
    summary_explaining_siblings,
)
from .batching import chunks
from .classify import CATEGORIES, FailureCategory
from .config import get_settings
from .models import (
    JOB_FAILED,
    REPORT_FAILED,
    REPORT_PENDING,
    REPORT_PROCESSED,
    Job,
    JobExecution,
    JobScoreHistory,
    Pipeline,
    Project,
    Repo,
    Report,
    TestCase,
    TestExecution,
    TestRun,
    TestScoreHistory,
    utcnow,
)
from .score_history import HISTORY_DAYS, TREND_DAYS, Trend, trend_for
from .scoring import FAILING

SortKey = Literal["score", "last_seen", "proven"]  # shared by the Test and Job leaderboards
NEW_FLAKY_DAYS = 7  # the "became flaky recently" window in the dashboard tiles


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


def issue_url(repo: str, number: int | None) -> str | None:
    """The GitHub issue a Test or Job was filed as, if any."""
    return f"https://github.com/{repo}/issues/{number}" if number is not None else None


def job_issue_url(repo: str, pipeline: Pipeline, job: Job) -> str | None:
    """The GitHub issue a Job was filed as; only GitHub Pipelines file issues."""
    return issue_url(repo, job.github_issue_number) if pipeline.provider == "github" else None


def to_test_out(tc: TestCase, project: str, repo: str, threshold: float, trend: Trend | None = None) -> schemas.TestOut:
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
        failure_category=tc.failure_category,
        clean_streak=tc.clean_streak,
        trend=trend,
        last_status=tc.last_status,
        last_seen_at=tc.last_seen_at,
        quarantined=tc.quarantined,
        quarantined_at=tc.quarantined_at,
        quarantine_reason=tc.quarantine_reason,
        quarantine_expires_at=tc.quarantine_expires_at,
        github_issue_number=tc.github_issue_number,
        github_issue_url=issue_url(repo, tc.github_issue_number),
    )


async def past_test_scores(db: AsyncSession, test_ids: list[int]) -> dict[int, float]:
    """Score of each Test's newest history row at least TREND_DAYS old."""
    return await _past_scores(db, TestScoreHistory, TestScoreHistory.test_case_id, test_ids)


async def _past_scores(db: AsyncSession, model, fk_col: ColumnElement[int], ids: list[int]) -> dict[int, float]:
    """Score of each entity's newest history row at least TREND_DAYS old.

    The one past-scores lookup, shared by the Test and Job halves: `model` is the
    Score-history model and `fk_col` its column back to the parent row.
    """
    cutoff = utcnow().date() - timedelta(days=TREND_DAYS)
    found: dict[int, float] = {}
    for chunk in chunks(ids):
        rn = func.row_number().over(partition_by=fk_col, order_by=model.day.desc()).label("rn")
        ranked = (
            select(fk_col.label("entity_id"), model.flakiness_score.label("score"), rn)
            .where(fk_col.in_(chunk), model.day <= cutoff)
            .subquery()
        )
        rows = await db.execute(select(ranked.c.entity_id, ranked.c.score).where(ranked.c.rn == 1))
        found.update(dict(rows.all()))
    return found


def _newest_history_before(model, fk_col: ColumnElement[int], parent_col: ColumnElement[int], days: int):
    """Lateral subquery: this row's newest Score history at least `days` old, or NULL (ADR 0007).

    The same lookup `past_test_scores`/`past_job_scores` do in Python, as a per-row SQL
    condition so a list query can filter with it before counting and paginating.
    """
    cutoff = utcnow().date() - timedelta(days=days)
    return (
        select(model.flakiness_score.label("score"))
        .where(fk_col == parent_col, model.day <= cutoff)
        .order_by(model.day.desc())
        .limit(1)
        .lateral()
    )


def _became_flaky_clause(past, threshold: float) -> ColumnElement[bool]:
    """True when the joined `_newest_history_before` row is missing or below the threshold."""
    return or_(past.c.score.is_(None), past.c.score < threshold)


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


def test_search_clause(text: str) -> ColumnElement[bool]:
    """Case-insensitive literal substring over name, classname and file."""
    pattern = f"%{escape_like(text)}%"
    return (
        TestCase.name.ilike(pattern, escape="\\")
        | TestCase.classname.ilike(pattern, escape="\\")
        | TestCase.file.ilike(pattern, escape="\\")
    )


@dataclass(frozen=True)
class LeaderboardEntity:
    """Everything the shared listing engine needs to know about one leaderboard entity (issue #62).

    Pure data: the entity's own columns, its Score-history model and the two columns that join
    it, and the per-entity callables. Nothing in here may encode a shared rule — the stable-hiding
    default, the threshold rule and the issue filter all live in `_list_leaderboard` only, and an
    intended per-entity difference is descriptor data (see `keeps_stable_rows` on TEST).
    """

    id: ColumnElement[int]
    score: ColumnElement[float]
    last_seen: ColumnElement[Any]
    proven: ColumnElement[int]
    issue_number: ColumnElement[int | None]
    history_model: type
    history_fk: ColumnElement[int]
    history_parent: ColumnElement[int]
    past_scores: Callable[[AsyncSession, list[int]], Awaitable[dict[int, float]]]
    search_clause: Callable[[str], ColumnElement[bool]]
    to_out: Callable[..., Any]
    page: type
    id_of: Callable[[Any], int]
    score_of: Callable[[Any], float]
    apply_filters: Callable[[Select, Any], Select]
    keeps_stable_rows: Callable[[Any], bool]


@dataclass(frozen=True)
class TestListFilters:
    """The Tests-only listing filters the engine must not know about; built by `list_tests`."""

    file: str | None = None
    category: FailureCategory | None = None
    quarantined: bool | None = None


def _test_filters(stmt: Select, f: TestListFilters) -> Select:
    if f.file:
        stmt = stmt.where(TestCase.file.ilike(f"%{escape_like(f.file)}%", escape="\\"))
    if f.category:
        stmt = stmt.where(TestCase.failure_category == f.category)
    if f.quarantined is not None:
        stmt = stmt.where(TestCase.quarantined.is_(f.quarantined))
    return stmt


TEST = LeaderboardEntity(
    id=TestCase.id,
    score=TestCase.flakiness_score,
    last_seen=TestCase.last_seen_at,
    proven=TestCase.confirmed_flake_count,
    issue_number=TestCase.github_issue_number,
    history_model=TestScoreHistory,
    history_fk=TestScoreHistory.test_case_id,
    history_parent=TestCase.id,
    past_scores=past_test_scores,
    search_clause=test_search_clause,
    to_out=to_test_out,
    page=schemas.TestPage,
    id_of=lambda tc: tc.id,
    score_of=lambda tc: tc.flakiness_score,
    apply_filters=_test_filters,
    # Tests keep score <= 0 rows visible when the caller asked for quarantined ones: Quarantine
    # deliberately covers stable Tests, so the Tests list has to show them. Jobs have no Quarantine
    # at all (ADR 0004: "Jobs have no Quarantine. A runner can't skip a job"), so this exemption is
    # Tests-only and lives here, as descriptor data, and nowhere else.
    keeps_stable_rows=lambda f: f.quarantined is True,
)


async def _list_leaderboard(
    db: AsyncSession,
    entity: LeaderboardEntity,
    stmt: Select,
    filters: Any,
    *,
    threshold: float,
    include_stable: bool,
    flaky_only: bool,
    sort: SortKey,
    page: int,
    page_size: int,
    q: str | None,
    has_issue: bool | None,
    new_within_days: int | None,
) -> Any:
    """The one Test/Job leaderboard listing skeleton (issue #62); `stmt` arrives already scoped.

    `page` (the page schema), `to_out` (the row mapper), `past_scores`, `search_clause`,
    `apply_filters` and `keeps_stable_rows` all come from `entity`; the shared policy below is
    stated here and nowhere else, so a new filter or sort key extends a descriptor instead of
    being re-implemented per entity.
    """
    # The single marked decision point: the threshold rule, and the stable-hiding default whose
    # per-entity difference (Tests exempt a quarantined listing; ADR 0004 gives Jobs no Quarantine)
    # is `entity.keeps_stable_rows`, i.e. descriptor data rather than a divergent branch.
    if flaky_only or new_within_days is not None:
        stmt = stmt.where(entity.score >= threshold)
    elif not include_stable and not q and not entity.keeps_stable_rows(filters):
        stmt = stmt.where(entity.score > 0)
    stmt = entity.apply_filters(stmt, filters)
    if q:
        stmt = stmt.where(entity.search_clause(q))
    if has_issue is not None:
        stmt = stmt.where(entity.issue_number.is_not(None) if has_issue else entity.issue_number.is_(None))
    if new_within_days is not None:
        oldest = _newest_history_before(entity.history_model, entity.history_fk, entity.history_parent, new_within_days)
        stmt = stmt.outerjoin(oldest, true()).where(_became_flaky_clause(oldest, threshold))

    total = (await db.execute(select(func.count()).select_from(stmt.subquery()))).scalar_one()

    order = {
        "score": (entity.score.desc(), entity.last_seen.desc(), entity.id),
        "last_seen": (entity.last_seen.desc(), entity.id.desc()),
        "proven": (entity.proven.desc(), entity.score.desc(), entity.id),
    }[sort]
    rows = (await db.execute(stmt.order_by(*order).offset((page - 1) * page_size).limit(page_size))).all()
    # Rows are the descriptor's base-select shape, (entity, parent, repo); the mapper unpacks them.
    ids = [entity.id_of(row[0]) for row in rows]
    past = await entity.past_scores(db, ids)
    return entity.page(
        items=[
            entity.to_out(*row, threshold, trend=trend_for(entity.score_of(row[0]), past.get(entity_id)))
            for row, entity_id in zip(rows, ids, strict=True)
        ],
        total=total,
        page=page,
        page_size=page_size,
    )


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
    category: FailureCategory | None = None,
    q: str | None = None,
    quarantined: bool | None = None,
    has_issue: bool | None = None,
    new_within_days: int | None = None,
) -> schemas.TestPage:
    """The Tests leaderboard: a thin wrapper over the shared engine (issue #62).

    The Scope-vs-repo difference between the two leaderboards is resolved here, not in the
    descriptor: `stmt` arrives already scoped. Signature and return schema are unchanged.
    """
    stmt = _scoped(tests_select(), scope)
    return await _list_leaderboard(
        db,
        TEST,
        stmt,
        TestListFilters(file=file, category=category, quarantined=quarantined),
        threshold=threshold,
        include_stable=include_stable,
        flaky_only=flaky_only,
        sort=sort,
        page=page,
        page_size=page_size,
        q=q,
        has_issue=has_issue,
        new_within_days=new_within_days,
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
    oldest = _newest_history_before(TestScoreHistory, TestScoreHistory.test_case_id, TestCase.id, NEW_FLAKY_DAYS)
    new_flaky = (
        await db.execute(
            tests_count(TestCase.flakiness_score >= threshold)
            .outerjoin(oldest, true())
            .where(_became_flaky_clause(oldest, threshold))
        )
    ).scalar_one()
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
    rows = await db.execute(
        _scoped(
            select(TestCase.failure_category, func.count(TestCase.id))
            .join(Project, TestCase.project_id == Project.id)
            .join(Repo, Project.repo_id == Repo.id)
            .where(TestCase.flakiness_score > 0, TestCase.failure_category.is_not(None))
            .group_by(TestCase.failure_category),
            scope,
        )
    )
    category_counts = {c: 0 for c in CATEGORIES} | {cat: n for cat, n in rows.all() if cat in CATEGORIES}
    return schemas.SummaryOut(
        total_tests=total,
        flaky_tests=flaky,
        suspect_tests=suspect,
        confirmed_flaky_tests=confirmed,
        new_flaky_7d=new_flaky,
        total_runs=runs,
        total_executions=executions,
        flake_threshold=threshold,
        category_counts=category_counts,
    )


# --- Test detail and quarantine (task 09) -------------------------------


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
            attempt=e.attempt,
            failure_category=e.failure_category,
        )
        for e, r in rows
    ]
    jobs = await _test_job_links(db, repo, {r.ci_job_id for _, r in rows if r.ci_job_id})

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
    past = await past_test_scores(db, [tc.id])
    history_rows = (
        (
            await db.execute(
                select(TestScoreHistory)
                .where(
                    TestScoreHistory.test_case_id == test_id,
                    TestScoreHistory.day > utcnow().date() - timedelta(days=HISTORY_DAYS),
                )
                .order_by(TestScoreHistory.day)
            )
        )
        .scalars()
        .all()
    )
    return schemas.HistoryOut(
        test=to_test_out(tc, project, repo, threshold, trend=trend_for(tc.flakiness_score, past.get(tc.id))),
        location=location,
        last_failing_sha=last_sha,
        last_failing_branch=last_branch,
        executions=executions,
        jobs=jobs,
        score_history=[
            schemas.ScorePointOut(
                day=h.day,
                flakiness_score=h.flakiness_score,
                confirmed_flake_count=h.confirmed_flake_count,
                executions=h.executions,
                failures=h.failures,
            )
            for h in history_rows
        ],
    )


async def set_quarantine(
    db: AsyncSession,
    test_id: int,
    quarantined: bool,
    *,
    reason: str | None = None,
    expires_in_days: int | None = None,
    threshold: float,
) -> schemas.TestOut | None:
    row = (await db.execute(tests_select().where(TestCase.id == test_id))).first()
    if row is None:
        return None
    tc, project, repo = row
    tc.quarantined = quarantined
    tc.quarantined_at = utcnow() if quarantined else None
    # Every write stores the body's values, so re-quarantining without a reason
    # or expiry clears the old ones instead of keeping a stale decision.
    tc.quarantine_reason = reason if quarantined else None
    tc.quarantine_expires_at = (
        utcnow() + timedelta(days=expires_in_days) if quarantined and expires_in_days is not None else None
    )
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
            reason=tc.quarantine_reason,
            expires_at=tc.quarantine_expires_at,
        )
        for tc in rows
    ]


# --- Search and name lookup (task 10, used by the MCP server) ----------


async def search_tests(
    db: AsyncSession, scope: Scope, query: str, *, threshold: float, limit: int = 20
) -> list[schemas.TestOut]:
    """Case-insensitive literal substring over name, classname and file; worst first."""
    stmt = _scoped(tests_select(), scope).where(test_search_clause(query))
    rows = (await db.execute(stmt.order_by(TestCase.flakiness_score.desc(), TestCase.id).limit(limit))).all()
    past = await past_test_scores(db, [tc.id for tc, _, _ in rows])
    return [
        to_test_out(tc, project, repo, threshold, trend=trend_for(tc.flakiness_score, past.get(tc.id)))
        for tc, project, repo in rows
    ]


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
        attempt=e.attempt,
        failure_category=e.failure_category,
    )


# --- Jobs (task 06) ------------------------------------------------------


def to_job_out(
    job: Job, pipeline: Pipeline, repo: Repo, threshold: float, trend: Trend | None = None
) -> schemas.JobOut:
    return schemas.JobOut(
        id=job.id,
        repo=repo.name,
        provider=pipeline.provider,
        pipeline=pipeline.name,
        name=job.name,
        flakiness_score=job.flakiness_score,
        tier=tier_for(job.flakiness_score, threshold),
        confirmed_flake_count=job.confirmed_flake_count,
        clean_streak=job.clean_streak,
        trend=trend,
        last_status=job.last_status,
        last_seen_at=job.last_seen_at,
        summary=job.summary,
        github_issue_number=job.github_issue_number,
        github_issue_url=job_issue_url(repo.name, pipeline, job),
    )


async def past_job_scores(db: AsyncSession, job_ids: list[int]) -> dict[int, float]:
    """Score of each Job's newest history row at least TREND_DAYS old."""
    return await _past_scores(db, JobScoreHistory, JobScoreHistory.job_id, job_ids)


def jobs_select() -> Select:
    """SELECT Job, Pipeline, Repo — the row shape to_job_out() takes."""
    return (
        select(Job, Pipeline, Repo)
        .join(Pipeline, Job.pipeline_id == Pipeline.id)
        .join(Repo, Pipeline.repo_id == Repo.id)
    )


def job_search_clause(text: str) -> ColumnElement[bool]:
    """Case-insensitive literal substring over Job name and Pipeline name."""
    pattern = f"%{escape_like(text)}%"
    return (Job.name.ilike(pattern, escape="\\")) | (Pipeline.name.ilike(pattern, escape="\\"))


async def list_jobs(
    db: AsyncSession,
    repo: str | None,
    *,
    threshold: float,
    include_stable: bool = False,
    flaky_only: bool = False,
    sort: SortKey = "score",
    page: int = 1,
    page_size: int = 50,
    q: str | None = None,
    pipeline: str | None = None,
    has_issue: bool | None = None,
    new_within_days: int | None = None,
) -> schemas.JobPage:
    stmt = jobs_select()
    if repo is not None:
        stmt = stmt.where(Repo.name == repo)
    if flaky_only or new_within_days is not None:
        stmt = stmt.where(Job.flakiness_score >= threshold)
    elif not include_stable and not q:
        stmt = stmt.where(Job.flakiness_score > 0)
    if q:
        stmt = stmt.where(job_search_clause(q))
    if pipeline is not None:
        stmt = stmt.where(Pipeline.name == pipeline)
    if has_issue is not None:
        stmt = stmt.where(Job.github_issue_number.is_not(None) if has_issue else Job.github_issue_number.is_(None))
    if new_within_days is not None:
        oldest = _newest_history_before(JobScoreHistory, JobScoreHistory.job_id, Job.id, new_within_days)
        stmt = stmt.outerjoin(oldest, true()).where(_became_flaky_clause(oldest, threshold))

    total = (await db.execute(select(func.count()).select_from(stmt.subquery()))).scalar_one()

    order = {
        "score": (Job.flakiness_score.desc(), Job.last_seen_at.desc(), Job.id),
        "last_seen": (Job.last_seen_at.desc(), Job.id.desc()),
        "proven": (Job.confirmed_flake_count.desc(), Job.flakiness_score.desc(), Job.id),
    }[sort]
    rows = (await db.execute(stmt.order_by(*order).offset((page - 1) * page_size).limit(page_size))).all()
    past = await past_job_scores(db, [job.id for job, _, _ in rows])
    return schemas.JobPage(
        items=[
            to_job_out(job, pipeline, repo, threshold, trend=trend_for(job.flakiness_score, past.get(job.id)))
            for job, pipeline, repo in rows
        ],
        total=total,
        page=page,
        page_size=page_size,
    )


async def job_summary(db: AsyncSession, repo: str | None, *, threshold: float) -> schemas.JobSummaryOut:
    def jobs_count(*conditions) -> Select:
        stmt = (
            select(func.count(Job.id))
            .join(Pipeline, Job.pipeline_id == Pipeline.id)
            .join(Repo, Pipeline.repo_id == Repo.id)
            .where(*conditions)
        )
        if repo is not None:
            stmt = stmt.where(Repo.name == repo)
        return stmt

    total = (await db.execute(jobs_count())).scalar_one()
    flaky = (await db.execute(jobs_count(Job.flakiness_score >= threshold))).scalar_one()
    suspect = (await db.execute(jobs_count(Job.flakiness_score > 0, Job.flakiness_score < threshold))).scalar_one()
    confirmed = (await db.execute(jobs_count(Job.confirmed_flake_count > 0))).scalar_one()
    oldest = _newest_history_before(JobScoreHistory, JobScoreHistory.job_id, Job.id, NEW_FLAKY_DAYS)
    new_flaky = (
        await db.execute(
            jobs_count(Job.flakiness_score >= threshold)
            .outerjoin(oldest, true())
            .where(_became_flaky_clause(oldest, threshold))
        )
    ).scalar_one()

    exec_stmt = (
        select(func.count(JobExecution.id))
        .join(Job, JobExecution.job_id == Job.id)
        .join(Pipeline, Job.pipeline_id == Pipeline.id)
        .join(Repo, Pipeline.repo_id == Repo.id)
    )
    if repo is not None:
        exec_stmt = exec_stmt.where(Repo.name == repo)
    total_exec = (await db.execute(exec_stmt)).scalar_one()

    pipe_stmt = select(Pipeline.name).join(Repo, Pipeline.repo_id == Repo.id)
    if repo is not None:
        pipe_stmt = pipe_stmt.where(Repo.name == repo)
    pipelines = list((await db.execute(pipe_stmt.distinct().order_by(Pipeline.name))).scalars())

    return schemas.JobSummaryOut(
        total_jobs=total,
        flaky_jobs=flaky,
        suspect_jobs=suspect,
        confirmed_flaky_jobs=confirmed,
        new_flaky_7d=new_flaky,
        total_job_executions=total_exec,
        flake_threshold=threshold,
        pipelines=pipelines,
    )


RUNNER_STATS_LIMIT = 10  # groups returned by `get_job`, worst unexplained rate first


async def _runner_stats(db: AsyncSession, job_id: int) -> list[schemas.RunnerStatOut]:
    """Job executions grouped by runner over the Job's newest score_window executions.

    One group per sorted, deduplicated label set, plus one per runner_name that appears
    on 2 or more executions (GitHub-hosted names are unique per run, so only repeated,
    typically self-hosted, names group). Unexplained means the same as everywhere
    else: failed and not `job_execution_explained()`.
    """
    windowed = job_execution_window_ids([job_id], get_settings().score_window).subquery()
    rows = (
        await db.execute(
            select(
                JobExecution.runner_name,
                JobExecution.runner_labels,
                JobExecution.status,
                job_execution_explained().label("explained"),
            )
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(JobExecution.id.in_(select(windowed.c.id)))
        )
    ).all()

    label_counts: dict[tuple[str, ...], list[int]] = defaultdict(lambda: [0, 0])  # [executions, unexplained]
    name_counts: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    name_labels: dict[str, set[str]] = defaultdict(set)
    for runner_name, runner_labels, status, explained in rows:
        # A repeated entry (schemas.py accepts duplicates) is the same label set.
        labels = tuple(sorted(set(runner_labels or [])))
        unexplained = 1 if status == JOB_FAILED and not explained else 0
        label_counts[labels][0] += 1
        label_counts[labels][1] += unexplained
        if runner_name:
            name_counts[runner_name][0] += 1
            name_counts[runner_name][1] += unexplained
            name_labels[runner_name].update(labels)

    stats = [(labels, None, counts[0], counts[1]) for labels, counts in label_counts.items()] + [
        (tuple(sorted(name_labels[name])), name, counts[0], counts[1])
        for name, counts in name_counts.items()
        if counts[0] >= 2
    ]
    stats.sort(key=lambda s: (s[3] / s[2], s[2], s[0], s[1] or ""), reverse=True)
    return [
        schemas.RunnerStatOut(labels=list(labels), runner_name=name, executions=execs, unexplained_failures=unexp)
        for labels, name, execs, unexp in stats[:RUNNER_STATS_LIMIT]
    ]


async def get_job(
    db: AsyncSession, job_id: int, *, threshold: float, executions_limit: int = 60
) -> schemas.JobHistoryOut | None:
    row = (await db.execute(jobs_select().where(Job.id == job_id))).first()
    if row is None:
        return None
    job, pipeline, repo = row

    execs = (
        (
            await db.execute(
                select(JobExecution)
                .where(JobExecution.job_id == job_id)
                .order_by(JobExecution.id.desc())
                .limit(executions_limit)
            )
        )
        .scalars()
        .all()
    )
    explained = await explained_ci_job_ids(db, repo.id, [e.ci_job_id for e in execs])
    runners = await _runner_stats(db, job_id)

    failed_ci_to_exec: dict[str, int] = {}  # ci_job_id -> job_execution id (unique within a Job)
    exec_outs: list[schemas.JobExecutionOut] = []
    unexplained = explained_failures = 0
    for e in execs:
        if e.status == JOB_FAILED:
            if e.ci_job_id in explained:
                outcome = "explained"
                explained_failures += 1
                failed_ci_to_exec[e.ci_job_id] = e.id
            else:
                outcome = "failed"
                unexplained += 1
        else:
            outcome = e.status  # passed | skipped
        exec_outs.append(
            schemas.JobExecutionOut(
                id=e.id,
                status=e.status,
                outcome=outcome,
                commit_sha=e.commit_sha,
                branch=e.branch,
                ci_run_id=e.ci_run_id,
                ci_run_attempt=e.ci_run_attempt,
                ci_job_id=e.ci_job_id,
                url=e.url,
                runner_name=e.runner_name,
                runner_labels=e.runner_labels,
                started_at=e.started_at,
                completed_at=e.completed_at,
                created_at=e.created_at,
                explained_by=[],
                explained_by_jobs=[],
            )
        )

    if failed_ci_to_exec:
        rows = (
            await db.execute(
                select(TestExecution, TestRun.ci_job_id, Project.name, TestCase.classname, TestCase.name)
                .join(TestRun, TestExecution.test_run_id == TestRun.id)
                .join(Project, Project.id == TestRun.project_id)
                .join(TestCase, TestExecution.test_case_id == TestCase.id)
                .where(
                    TestRun.ci_job_id.in_(set(failed_ci_to_exec)),
                    Project.repo_id == repo.id,
                    TestExecution.status.in_(FAILING),
                )
                .order_by(TestExecution.id.desc())
            )
        ).all()
        by_exec: dict[int, list[schemas.ExplainingTestOut]] = {eid: [] for eid in failed_ci_to_exec.values()}
        for te, ci, project, classname, name in rows:
            eid = failed_ci_to_exec.get(ci)
            if eid is None or len(by_exec[eid]) >= 20:
                continue
            by_exec[eid].append(
                schemas.ExplainingTestOut(
                    test_id=te.test_case_id, project=project, classname=classname, name=name, status=te.status
                )
            )
        for j in exec_outs:
            j.explained_by = by_exec.get(j.id, [])

    if job.summary:
        # A summary Job runs no Tests, so its explained executions are explained by
        # the failed non-summary Jobs of the same CI run attempt. attribution owns
        # that rule — including the empty-run-id guard — so the "explained" outcome
        # and the Jobs listed beside it cannot drift apart.
        pairs = {(e.ci_run_id, e.ci_run_attempt) for e in execs if e.status == JOB_FAILED and e.ci_job_id in explained}
        by_pair = await summary_explaining_siblings(db, repo.id, pairs)
        for j in exec_outs:
            if j.outcome == "explained":
                j.explained_by_jobs = [
                    schemas.ExplainingJobOut(job_id=jid, pipeline=pipe, name=jname, url=url)
                    for jid, pipe, jname, url in by_pair.get((j.ci_run_id, j.ci_run_attempt), [])
                ]

    past = await past_job_scores(db, [job.id])
    history_rows = (
        (
            await db.execute(
                select(JobScoreHistory)
                .where(
                    JobScoreHistory.job_id == job_id,
                    JobScoreHistory.day > utcnow().date() - timedelta(days=HISTORY_DAYS),
                )
                .order_by(JobScoreHistory.day)
            )
        )
        .scalars()
        .all()
    )
    return schemas.JobHistoryOut(
        job=to_job_out(job, pipeline, repo, threshold, trend=trend_for(job.flakiness_score, past.get(job.id))),
        unexplained_failures=unexplained,
        explained_failures=explained_failures,
        executions=exec_outs,
        runners=runners,
        score_history=[
            schemas.ScorePointOut(
                day=h.day,
                flakiness_score=h.flakiness_score,
                confirmed_flake_count=h.confirmed_flake_count,
                executions=h.executions,
                failures=h.failures,
            )
            for h in history_rows
        ],
    )


async def search_jobs(
    db: AsyncSession, repo: str, query: str, *, threshold: float, limit: int = 20
) -> list[schemas.JobOut]:
    """Case-insensitive literal substring over Job and Pipeline name; worst first."""
    stmt = jobs_select().where(
        Repo.name == repo,
        job_search_clause(query),
    )
    rows = (await db.execute(stmt.order_by(Job.flakiness_score.desc(), Job.id).limit(limit))).all()
    past = await past_job_scores(db, [job.id for job, _, _ in rows])
    return [
        to_job_out(job, pipeline, reponame, threshold, trend=trend_for(job.flakiness_score, past.get(job.id)))
        for job, pipeline, reponame in rows
    ]


async def find_job_ids(db: AsyncSession, repo: str, name: str, pipeline: str | None = None) -> list[int]:
    """Exact-match lookup of a Job by name (and optionally Pipeline name)."""
    stmt = (
        select(Job.id)
        .join(Pipeline, Job.pipeline_id == Pipeline.id)
        .join(Repo, Pipeline.repo_id == Repo.id)
        .where(Repo.name == repo, Job.name == name)
    )
    if pipeline is not None:
        stmt = stmt.where(Pipeline.name == pipeline)
    return list((await db.execute(stmt.order_by(Job.id))).scalars())


async def _test_job_links(db: AsyncSession, repo_name: str, ci_job_ids: set[str]) -> list[schemas.TestJobLinkOut]:
    """Distinct Jobs whose executions share a ci_job_id with the given Runs, in this Repo."""
    if not ci_job_ids:
        return []
    rows = (
        await db.execute(
            select(Job.id, Pipeline.name, Job.name)
            .select_from(JobExecution)
            .join(Job, JobExecution.job_id == Job.id)
            .join(Pipeline, Job.pipeline_id == Pipeline.id)
            .join(Repo, Pipeline.repo_id == Repo.id)
            .where(Repo.name == repo_name, JobExecution.ci_job_id.in_(ci_job_ids))
            .distinct()
            .order_by(Job.name, Job.id)
        )
    ).all()
    return [schemas.TestJobLinkOut(job_id=jid, pipeline=p, name=n) for jid, p, n in rows]


# --- CI run failures (issue #5) -----------------------------------------

FAILURES_LIMIT = 500
CI_RUN_MESSAGE_MAX = 1024


class CiRunNotFound(LookupError):
    """The Repo, CI run or CI run attempt is unknown; str(exc) is the 404 detail."""


async def ci_run_failures(
    db: AsyncSession,
    repo: str,
    ci_run_id: str,
    ci_run_attempt: int | None,
    *,
    threshold: float,
    limit: int = FAILURES_LIMIT,
) -> schemas.CiRunFailuresOut:
    """The failing Tests and failed Jobs of one CI run attempt (issue #5).

    `repo` is already normalized. ci_run_attempt=None means the highest known attempt.
    Raises CiRunNotFound.
    """
    repo_row = (await db.execute(select(Repo).where(Repo.name == repo))).scalar_one_or_none()
    if repo_row is None:
        raise CiRunNotFound("Repo not found")
    attempts = await _ci_run_attempts(db, repo_row.id, ci_run_id)
    if not attempts:
        raise CiRunNotFound("CI run not found")
    if ci_run_attempt is None:
        ci_run_attempt = max(attempts)
    elif ci_run_attempt not in attempts:
        raise CiRunNotFound("CI run attempt not found")
    failures, total = await _ci_run_test_failures(
        db, repo_row, ci_run_id, ci_run_attempt, threshold=threshold, limit=limit
    )
    return schemas.CiRunFailuresOut(
        repo=repo_row.name,
        ci_run_id=ci_run_id,
        ci_run_attempt=ci_run_attempt,
        flake_threshold=threshold,
        reports=await _ci_run_report_counts(db, repo_row.id, ci_run_id, ci_run_attempt),
        total_failures=total,
        truncated=total > len(failures),
        failures=failures,
        failed_jobs=await _ci_run_failed_jobs(db, repo_row, ci_run_id, ci_run_attempt, threshold=threshold),
    )


async def _ci_run_attempts(db: AsyncSession, repo_id: int, ci_run_id: str) -> set[int]:
    """Every known attempt of one CI run in one Repo (NULL counts as 1)."""
    reports = await db.execute(
        select(func.coalesce(Report.ci_run_attempt, 1))
        .where(Report.repo_id == repo_id, Report.ci_run_id == ci_run_id)
        .distinct()
    )
    runs = await db.execute(
        select(func.coalesce(TestRun.ci_run_attempt, 1))
        .join(Project, Project.id == TestRun.project_id)
        .where(Project.repo_id == repo_id, TestRun.ci_run_id == ci_run_id)
        .distinct()
    )
    jobs = await db.execute(
        select(JobExecution.ci_run_attempt)
        .join(Job, Job.id == JobExecution.job_id)
        .join(Pipeline, Pipeline.id == Job.pipeline_id)
        .where(Pipeline.repo_id == repo_id, JobExecution.ci_run_id == ci_run_id)
        .distinct()
    )
    return set(reports.scalars()) | set(runs.scalars()) | set(jobs.scalars())


async def _ci_run_report_counts(
    db: AsyncSession, repo_id: int, ci_run_id: str, ci_run_attempt: int
) -> schemas.ReportCountsOut:
    """Counts of both Report kinds for one Repo and CI run attempt."""
    rows = await db.execute(
        select(Report.status, func.count())
        .where(
            Report.repo_id == repo_id,
            Report.ci_run_id == ci_run_id,
            func.coalesce(Report.ci_run_attempt, 1) == ci_run_attempt,
        )
        .group_by(Report.status)
    )
    counts = dict(rows.all())
    return schemas.ReportCountsOut(
        pending=counts.get(REPORT_PENDING, 0),
        processed=counts.get(REPORT_PROCESSED, 0),
        failed=counts.get(REPORT_FAILED, 0),
    )


async def _ci_run_test_failures(
    db: AsyncSession, repo: Repo, ci_run_id: str, ci_run_attempt: int, *, threshold: float, limit: int
) -> tuple[list[schemas.CiRunTestFailureOut], int]:
    """Failing (Test, Run) pairs of one CI run attempt, capped at `limit`; also the uncapped count."""
    run_ids = (
        select(TestRun.id)
        .join(Project, Project.id == TestRun.project_id)
        .where(
            Project.repo_id == repo.id,
            TestRun.ci_run_id == ci_run_id,
            func.coalesce(TestRun.ci_run_attempt, 1) == ci_run_attempt,
        )
        .scalar_subquery()
    )
    pairs = (
        select(TestExecution.test_case_id, TestExecution.test_run_id)
        .where(TestExecution.test_run_id.in_(run_ids), TestExecution.status.in_(FAILING))
        .distinct()
        .subquery()
    )
    total = (await db.execute(select(func.count()).select_from(pairs))).scalar_one()
    if total == 0:
        return [], 0

    page = (
        await db.execute(
            select(TestCase, Project.name, TestRun.id, TestRun.ci_job_id)
            .select_from(pairs)
            .join(TestCase, TestCase.id == pairs.c.test_case_id)
            .join(TestRun, TestRun.id == pairs.c.test_run_id)
            .join(Project, Project.id == TestCase.project_id)
            .order_by(Project.name, TestCase.suite, TestCase.classname, TestCase.name, TestRun.id)
            .limit(limit)
        )
    ).all()
    # Both IN lists hold at most `limit` ids, so no chunking is needed here.
    executions = (
        (
            await db.execute(
                select(TestExecution)
                .where(
                    TestExecution.test_run_id.in_({run_id for _, _, run_id, _ in page}),
                    TestExecution.test_case_id.in_({tc.id for tc, _, _, _ in page}),
                )
                .order_by(TestExecution.attempt, TestExecution.id)
            )
        )
        .scalars()
        .all()
    )
    by_pair: dict[tuple[int, int], list[TestExecution]] = {}
    for e in executions:
        by_pair.setdefault((e.test_case_id, e.test_run_id), []).append(e)

    items: list[schemas.CiRunTestFailureOut] = []
    for tc, project, run_id, ci_job_id in page:
        execs = by_pair[(tc.id, run_id)]  # the pair is failing, so it has at least one Execution
        failing = [e for e in execs if e.status in FAILING]
        last_failing = failing[-1]
        items.append(
            schemas.CiRunTestFailureOut(
                test_id=tc.id,
                project=project,
                suite=tc.suite,
                classname=tc.classname,
                name=tc.name,
                run_id=run_id,
                ci_job_id=ci_job_id,
                status=execs[-1].status,
                failed_attempts=len(failing),
                failure_category=last_failing.failure_category,
                message=last_failing.message[:CI_RUN_MESSAGE_MAX],
                tier=tier_for(tc.flakiness_score, threshold),
                flakiness_score=tc.flakiness_score,
                confirmed_flake_count=tc.confirmed_flake_count,
                clean_streak=tc.clean_streak,
                quarantined=tc.quarantined,
                github_issue_number=tc.github_issue_number,
                github_issue_url=issue_url(repo.name, tc.github_issue_number),
            )
        )
    return items, total


async def _ci_run_failed_jobs(
    db: AsyncSession, repo: Repo, ci_run_id: str, ci_run_attempt: int, *, threshold: float
) -> list[schemas.CiRunFailedJobOut]:
    """Failed Job executions of one CI run attempt, each explained or unexplained."""
    rows = (
        await db.execute(
            select(JobExecution, Job, Pipeline)
            .join(Job, Job.id == JobExecution.job_id)
            .join(Pipeline, Pipeline.id == Job.pipeline_id)
            .where(
                Pipeline.repo_id == repo.id,
                JobExecution.ci_run_id == ci_run_id,
                JobExecution.ci_run_attempt == ci_run_attempt,
                JobExecution.status == JOB_FAILED,
            )
            .order_by(Pipeline.name, Job.name, JobExecution.id)
        )
    ).all()
    explained = await explained_ci_job_ids(db, repo.id, [e.ci_job_id for e, _, _ in rows])
    return [
        schemas.CiRunFailedJobOut(
            job_id=job.id,
            pipeline=pipeline.name,
            name=job.name,
            ci_job_id=e.ci_job_id,
            url=e.url,
            outcome="explained" if e.ci_job_id in explained else "unexplained",
            summary=job.summary,
            tier=tier_for(job.flakiness_score, threshold),
            flakiness_score=job.flakiness_score,
            confirmed_flake_count=job.confirmed_flake_count,
            clean_streak=job.clean_streak,
            github_issue_number=job.github_issue_number,
            github_issue_url=job_issue_url(repo.name, pipeline, job),
        )
        for e, job, pipeline in rows
    ]
