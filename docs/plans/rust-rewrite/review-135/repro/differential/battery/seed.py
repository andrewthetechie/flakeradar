"""The fixture both servers receive, built only through their own routes.

Every write goes to both servers, so the two databases hold the same rows with
the same ids: a `report_id` or a Test id from one answer must match the other's,
and `rows`/`dump` comparisons afterwards are meaningful. The fixture is chosen to
exercise every read filter: five Failure categories, Proven flakes, a broken Test,
a suspect, a stable, a skipped and a retried Test, two Projects, a second Repo,
Jobs that are explained and unexplained, a summary Job, a cancelled Job, a
quarantined Test and a linked issue.
"""

from __future__ import annotations

from xml.sax.saxutils import quoteattr

from harness import API_KEY, Battery

REPO = "acme/app"
OTHER = "acme/other"
PROJECT = "backend"
FRONTEND = "frontend"
PIPELINE = ".github/workflows/ci.yml"

FAILURES = {
    # name: (classname, file, line, message, details)
    "test_timing_flake": (
        "tests.test_cron",
        "tests/test_cron.py",
        42,
        "TimeoutError: timed out after 5s",
        'Traceback (most recent call last):\n  File "tests/test_cron.py", line 42\nTimeoutError: timed out after 5s',
    ),
    "test_network_flake": (
        "tests.test_api",
        "tests/test_api.py",
        7,
        "ConnectionResetError: [Errno 104] reset by peer",
        "Traceback (most recent call last):\nConnectionResetError: [Errno 104] reset by peer",
    ),
    "test_assert_flake": (
        "tests.test_math",
        "./tests/test_math.py",
        3,
        "AssertionError: expected 3, got 4",
        "assert 4 == 3",
    ),
    "test_env_flake": (
        "env",
        "env_test.go",
        None,
        'cannot find package "example.com/missing"',
        "go: cannot find package",
    ),
}
BROKEN = "test_broken"  # fails on c1..c5, never passes: broken on the Default branch
SUSPECT = "test_suspect"  # fails once on c8 only
STABLE = "test_stable"
SKIPPED = "test_skipped"
RETRIED = "test_retried"


def _case(name: str, *, classname: str = "misc", file: str | None = None, line: int | None = None,
          message: str | None = None, details: str = "", status: str = "passed",
          retries: list[tuple[str, str, str]] | None = None, time: str | None = None) -> str:
    attrs = " ".join(
        f"{key}={quoteattr(str(value))}"
        for key, value in {"name": name, "classname": classname, "file": file, "line": line, "time": time}.items()
        if value is not None
    )
    children = ""
    for retry_status, retry_message, retry_details in retries or []:
        tag = "flakyFailure" if retry_status == "failed" else "flakyError"
        children += f"<{tag} message={quoteattr(retry_message)}>{retry_details}</{tag}>"
    if status in ("failed", "error"):
        tag = "failure" if status == "failed" else "error"
        children += f"<{tag} message={quoteattr(message or '')}>{details}</{tag}>"
    elif status == "skipped":
        children += "<skipped/>"
    return f"<testcase {attrs}>{children}</testcase>" if children else f"<testcase {attrs}/>"


def upload(battery: Battery, cases: list[str], *, suite: str = "pytest", **query: str) -> None:
    """One JUnit upload, raw body as the writers-app `curl` sends it."""
    body = f"<testsuite name={quoteattr(suite)}>" + "".join(cases) + "</testsuite>"
    battery.call(
        "POST",
        "/api/ingest",
        params=[(key, str(value)) for key, value in query.items()],
        headers={**API_KEY, "Content-Type": "application/xml"},
        content=body.encode(),
        label=f"ingest {query}",
    )


def _fail_and_pass(battery: Battery, commit: str, **extra: str) -> None:
    """A failing upload then a passing one for the same commit: Proven flakes."""
    failed = [
        _case(name, classname=classname, file=path, line=line, message=message, details=details, status="failed")
        for name, (classname, path, line, message, details) in FAILURES.items()
    ]
    passed = [_case(name, classname=classname, file=path, line=line) for name, (classname, path, line, *_rest) in FAILURES.items()]
    upload(battery, failed, repo=REPO, project=PROJECT, commit_sha=commit, branch="main", **extra)
    upload(battery, passed, repo=REPO, project=PROJECT, commit_sha=commit, branch="main", **extra)


def pipeline(battery: Battery, run: str, jobs: list[dict], **extra: str) -> None:
    payload = {
        "repo": REPO,
        "provider": "github",
        "pipeline": PIPELINE,
        "commit_sha": "p1",
        "branch": "main",
        "ci_run_id": run,
        "jobs": jobs,
        **extra,
    }
    battery.call("POST", "/api/ingest/pipeline", headers=API_KEY, json_body=payload, label=f"pipeline {run}")


def seed(battery: Battery) -> None:
    """Build the fixture, draining between batches so both workers see one order."""
    battery.phase = "seed"
    # c1 also introduces the Default branch.
    _fail_and_pass(battery, "c1", default_branch="main", ci_run_id="run-c1")
    battery.drain()

    # c2..c4 continue the flips; the broken Test fails in every one of them.
    for commit in ("c2", "c3", "c4"):
        upload(
            battery,
            [_case(BROKEN, classname="broken", file="broken_test.go", message="panic: nil map", details="goroutine 1", status="failed")],
            repo=REPO,
            project=PROJECT,
            commit_sha=commit,
            branch="main",
        )
        _fail_and_pass(battery, commit, ci_run_id=f"run-{commit}")

    # A fifth failing commit for the broken Test (over the default `broken_streak`
    # of 5), plus a skipped Test and a retried one (attempt 0 failed, attempt 1 passed).
    upload(
        battery,
        [
            _case(BROKEN, classname="broken", file="broken_test.go", message="panic: nil map", status="failed"),
            _case(SKIPPED, classname="skipped", status="skipped"),
            _case(
                RETRIED,
                classname="retried",
                status="passed",
                retries=[("failed", "AssertionError: flaky first try", "assert 1 == 2")],
            ),
        ],
        repo=REPO,
        project=PROJECT,
        commit_sha="c5",
        branch="main",
    )
    # Six clean passes then one failure: a suspect, not a flake.
    for commit in ("c6", "c7", "c8"):
        failed = commit == "c8"
        upload(
            battery,
            [
                _case(STABLE, classname="tests.test_math", file="tests/test_math.py", line=9),
                _case(
                    SUSPECT,
                    classname="tests.test_slow",
                    file="tests/test_slow.py",
                    line=1,
                    message="TimeoutError: slow" if failed else None,
                    status="failed" if failed else "passed",
                ),
                # One name in two classnames: the MCP tools' ambiguity rule.
                _case("dup_name", classname="first", file="tests/test_dup.py", line=1),
                _case("dup_name", classname="second", file="tests/test_dup.py", line=9),
            ],
            repo=REPO,
            project=PROJECT,
            commit_sha=commit,
            branch="main",
        )
    battery.drain()

    # A second Project, with a `root` and a `line_base=0` report, and a feature
    # branch upload (which must not count as a Default-branch flip).
    for commit, branch in (("f1", "main"), ("f2", "feat/card")):
        upload(
            battery,
            [_case("renders a card", classname="Card", file="./src/Card.test.ts", line=0)],
            suite="jest",
            repo=REPO,
            project=FRONTEND,
            root="web",
            line_base="0",
            commit_sha=commit,
            branch=branch,
        )

    # A second Repo, so a scoped read is not accidentally the whole database.
    upload(
        battery,
        [_case("other_stable", classname="other")],
        repo=OTHER,
        project="default",
        commit_sha="o1",
        branch="main",
        default_branch="main",
    )
    battery.drain()

    # Pipeline reports: one CI run whose failed Job has a failing Test from the
    # same attempt (explained), one with no failing Test (unexplained, scored),
    # one summary Job with a failed non-summary Job (explained), one cancelled
    # Job (skipped), and a passing Job.
    pipeline(battery, "run-explain", [
        {"ci_job_id": "job-explain", "name": "backend", "status": "failed", "url": "https://ci/1",
         "runner_name": "runner-1", "runner_labels": ["ubuntu", "x64"]},
        {"ci_job_id": "job-ok", "name": "frontend", "status": "passed", "runner_name": "runner-2",
         "runner_labels": ["macos"]},
    ])
    upload(
        battery,
        [_case("explains_it", classname="explained", message="boom", status="failed")],
        repo=REPO,
        project=PROJECT,
        commit_sha="p1",
        branch="main",
        ci_run_id="run-explain",
        ci_job_id="job-explain",
        pipeline=PIPELINE,
    )
    pipeline(battery, "run-unexplained", [
        {"ci_job_id": "job-unexplained", "name": "lint", "status": "failed", "runner_labels": ["ubuntu"]},
    ])
    pipeline(battery, "run-summary", [
        {"ci_job_id": "job-gate", "name": "all-green", "status": "failed", "summary": True},
        {"ci_job_id": "job-unit", "name": "unit", "status": "failed"},
    ])
    pipeline(battery, "run-cancel", [
        {"ci_job_id": "job-cancel", "name": "slow", "status": "cancelled"},
    ])
    battery.drain()

    # A quarantine with a reason and an expiry, and an Issue link on another Test.
    test_id = test_id_of(battery, "test_timing_flake")
    battery.call(
        "POST",
        f"/api/tests/{test_id}/quarantine",
        json_body={"quarantined": True, "reason": "flaky since the 3.12 bump", "expires_in_days": 30},
        label="seed: quarantine a Test",
    )
    linked_id = test_id_of(battery, "test_network_flake")
    battery.call(
        "PUT",
        f"/api/tests/{linked_id}/issue",
        headers=API_KEY,
        json_body={"number": 4242, "url": "https://github.com/acme/app/issues/4242"},
        label="seed: link an issue",
    )
    battery.note(
        f"fixture: {REPO} ({PROJECT}, {FRONTEND}) and {OTHER} over commits c1-c8, one feature branch, "
        "four Pipeline runs, one quarantine and one Issue link"
    )
    battery.drain()


def test_id_of(battery: Battery, name: str) -> int:
    """The Test id both databases agree on, read from the reference's own rows."""
    return int(battery.psql("py", f"SELECT id FROM test_cases WHERE name = '{name}' ORDER BY id LIMIT 1").strip())


def job_id_of(battery: Battery, name: str) -> int:
    return int(battery.psql("py", f"SELECT id FROM jobs WHERE name = '{name}' ORDER BY id LIMIT 1").strip())


def run_id_of(battery: Battery, commit: str) -> int:
    return int(battery.psql("py", f"SELECT id FROM test_runs WHERE commit_sha = '{commit}' ORDER BY id LIMIT 1").strip())


def report_id_of(battery: Battery, status: str = "processed") -> int:
    return int(battery.psql("py", f"SELECT id FROM reports WHERE status = '{status}' ORDER BY id DESC LIMIT 1").strip())
