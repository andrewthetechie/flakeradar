"""Seed the FlakeRadar fixture that the summary action self-test asserts on.

Run against a throwaway database whose schema already exists (the backend
migrates on startup). It creates one CI run attempt with one Repo/Project and
two failed Tests of one Job:

  * `test_known_flaky`   -- tier ``flaky`` (score >= flake_threshold), 3 proven
                           flakes: FlakeRadar already knows it.
  * `test_new_failure`   -- tier ``stable`` (score 0), no proven flakes: a
                           failure the change may have caused.

Both share ``ci_job_id`` $CI_JOB_ID so the /api/ci-runs/.../failures endpoint
returns them for one job, and the summary action can write "1 of 2 failures are
known flaky tests". Removes any previous rows with the same fingerprints first,
so the script is idempotent.

Requires env: FLAKERADAR_DATABASE_URL, CI_RUN_ID, CI_RUN_ATTEMPT, CI_JOB_ID.
"""

import asyncio
import os
import sys
from pathlib import Path

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

# The backend packages live one directory up from the repo root.
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "backend"))

from app import models

FIXTURE_FINGERPRINTS = ("fx-known-flaky", "fx-new-failure")
KNOWN_FLAKY_SCORE = 0.9  # >= FLAKERADAR_FLAKE_THRESHOLD (default 0.30)


async def main() -> None:
    missing = [
        k for k in ("FLAKERADAR_DATABASE_URL", "CI_RUN_ID", "CI_RUN_ATTEMPT", "CI_JOB_ID") if not os.environ.get(k)
    ]
    if missing:
        sys.exit(f"missing env: {', '.join(missing)}")
    run_id = os.environ["CI_RUN_ID"]
    attempt = int(os.environ["CI_RUN_ATTEMPT"])
    job_id = os.environ["CI_JOB_ID"]

    engine = create_async_engine(os.environ["FLAKERADAR_DATABASE_URL"])
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            # Idempotent: clear any previous fixture rows.
            old = (await session.execute(select_test_ids())).scalars().all()
            if old:
                await session.execute(delete(models.TestExecution).where(models.TestExecution.test_case_id.in_(old)))
                await session.execute(delete(models.TestCase).where(models.TestCase.id.in_(old)))

            repo = models.Repo(name="andrewthetechie/flakeradar", default_branch="main")
            session.add(repo)
            await session.flush()
            project = models.Project(repo_id=repo.id, name="default", root="")
            session.add(project)
            await session.flush()

            known = models.TestCase(
                project_id=project.id,
                fingerprint="fx-known-flaky",
                suite="pytest",
                classname="tests.test_fixture",
                name="test_known_flaky",
                flakiness_score=KNOWN_FLAKY_SCORE,
                confirmed_flake_count=3,
            )
            new_failure = models.TestCase(
                project_id=project.id,
                fingerprint="fx-new-failure",
                suite="pytest",
                classname="tests.test_fixture",
                name="test_new_failure",
                flakiness_score=0.0,
            )
            session.add_all([known, new_failure])
            await session.flush()

            run = models.TestRun(
                project_id=project.id,
                commit_sha="a" * 40,
                branch="main",
                ci_run_id=run_id,
                ci_job_id=job_id,
                ci_run_attempt=attempt,
                pipeline=".github/workflows/ci.yml",
            )
            session.add(run)
            await session.flush()

            session.add_all(
                [
                    models.TestExecution(
                        test_case_id=known.id,
                        test_run_id=run.id,
                        status="failed",
                        message="boom (flaky)",
                        failure_category="assertion",
                    ),
                    models.TestExecution(
                        test_case_id=new_failure.id,
                        test_run_id=run.id,
                        status="failed",
                        message="boom (new)",
                        failure_category="assertion",
                    ),
                ]
            )
            await session.commit()
    finally:
        await engine.dispose()


def select_test_ids():
    from sqlalchemy import select

    return select(models.TestCase.id).where(models.TestCase.fingerprint.in_(FIXTURE_FINGERPRINTS))


if __name__ == "__main__":
    asyncio.run(main())
