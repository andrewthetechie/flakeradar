"""GitLab reporter sample (samples/gitlab/report_pipeline.py): status mapping and report shape.

Pure unit tests: they load the sample module from the repository root and exercise the
mapping functions, so no database and no network access is involved.
"""

import importlib.util
import inspect
from pathlib import Path

import pytest
from app.schemas import PipelineReportIn
from pydantic import ValidationError


def _sample():
    path = Path(__file__).resolve().parents[2] / "samples" / "gitlab" / "report_pipeline.py"
    spec = importlib.util.spec_from_file_location("report_pipeline", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sample = _sample()


def _job(job_id: int, name: str, status: str, **kw) -> dict:
    """One GitLab API job object, shaped like the response in docs.gitlab.com/api/jobs/."""
    d = {
        "id": job_id,
        "name": name,
        "status": status,
        "allow_failure": False,
        "web_url": f"https://gitlab.com/acme/app/-/jobs/{job_id}",
        "runner": {"description": "runner-1"},
        "tag_list": ["linux", "docker"],
        "started_at": "2026-10-26T09:00:00.000Z",
        "finished_at": "2026-10-26T09:01:02.722Z",
    }
    d.update(kw)
    return d


ENV = {
    "CI_API_V4_URL": "https://gitlab.com/api/v4",
    "CI_PROJECT_ID": "42",
    "CI_PROJECT_PATH": "acme/app",
    "CI_PIPELINE_ID": "541",
    "CI_JOB_ID": "112",
    "CI_COMMIT_SHA": "deadbeefcafebabe0123456789abcdef01234567",
    "CI_COMMIT_REF_NAME": "feature/flaky",
    "CI_DEFAULT_BRANCH": "main",
}

# Sorted by job id descending, as the Jobs API returns it. 102 and 110 are a retried pair:
# the same Job name, two job ids in the SAME pipeline, fail then pass on one SHA.
PIPELINE_JOBS = [
    _job(112, "flakeradar:report", "running", runner=None, tag_list=[], started_at=None, finished_at=None),
    _job(110, "test", "success"),
    _job(108, "manual deploy", "manual", runner=None, tag_list=[], finished_at=None),
    _job(106, "deploy", "canceled"),
    _job(104, "lint", "failed", allow_failure=True),
    _job(102, "test", "failed"),
    _job(100, "build", "success"),
]


def test_status_map_is_the_documented_mapping():
    assert sample.STATUS_MAP == {
        "success": "passed",
        "failed": "failed",
        "canceled": "skipped",
        "skipped": "skipped",
        "manual": "skipped",
    }


def test_sample_exposes_the_planned_api():
    assert list(inspect.signature(sample.map_job).parameters) == ["job"]
    assert list(inspect.signature(sample.build_report).parameters) == ["jobs", "env"]
    assert list(inspect.signature(sample.fetch_jobs).parameters) == ["api_base", "project_id", "pipeline_id", "token"]
    post = inspect.signature(sample.post_report).parameters
    assert list(post) == ["base_url", "api_token", "payload", "attempts"]
    assert post["attempts"].default == 5
    assert list(inspect.signature(sample.main).parameters) == []


def test_build_report_maps_every_reportable_job():
    payload = sample.build_report(PIPELINE_JOBS, ENV)
    jobs = {j["ci_job_id"]: j for j in payload["jobs"]}

    # success -> passed, failed (also with allow_failure) -> failed, canceled/manual -> skipped.
    assert jobs["100"]["status"] == "passed"
    assert jobs["110"]["status"] == "passed"
    assert jobs["102"]["status"] == "failed"
    assert jobs["104"]["status"] == "failed"
    assert jobs["106"]["status"] == "skipped"
    assert jobs["108"]["status"] == "skipped"
    # Unmapped statuses are left out, which is how the running reporter job is excluded.
    assert "112" not in jobs
    assert sorted(jobs) == sorted(["100", "102", "104", "106", "108", "110"])


def test_build_report_keeps_both_tries_of_a_retried_job():
    payload = sample.build_report(PIPELINE_JOBS, ENV)
    tests = [j for j in payload["jobs"] if j["name"] == "test"]
    # A retry is a new job id in the same pipeline, so both appear: a proven flake.
    # The payload is chronological (oldest try first), so the backend's newest-wins
    # semantics keep the passing retry as the Job's last_status.
    assert [(j["ci_job_id"], j["status"]) for j in tests] == [("102", "failed"), ("110", "passed")]
    PipelineReportIn.model_validate(payload)


def test_build_report_top_level_fields_come_from_the_ci_environment():
    payload = sample.build_report(PIPELINE_JOBS, ENV)
    assert payload["repo"] == "acme/app"
    assert payload["provider"] == "gitlab"
    assert payload["pipeline"] == ".gitlab-ci.yml"
    assert payload["commit_sha"] == ENV["CI_COMMIT_SHA"]
    assert payload["branch"] == "feature/flaky"
    assert payload["default_branch"] == "main"
    assert payload["ci_run_id"] == "541"
    assert payload["ci_run_attempt"] == 1


def test_build_report_carries_job_fields_onto_the_job_result():
    payload = sample.build_report(PIPELINE_JOBS, ENV)
    retried = next(j for j in payload["jobs"] if j["ci_job_id"] == "110")
    assert retried == {
        "ci_job_id": "110",
        "name": "test",
        "status": "passed",
        "url": "https://gitlab.com/acme/app/-/jobs/110",
        "runner_name": "runner-1",
        "runner_labels": ["linux", "docker"],
        "started_at": "2026-10-26T09:00:00.000Z",
        "completed_at": "2026-10-26T09:01:02.722Z",
        "summary": False,
    }
    # A job with no runner yet (manual) still maps, with empty runner fields.
    manual = next(j for j in payload["jobs"] if j["ci_job_id"] == "108")
    assert manual["runner_name"] == ""
    assert manual["runner_labels"] == []
    assert manual["completed_at"] is None


def test_summary_jobs_are_flagged_in_the_payload(monkeypatch):
    monkeypatch.setattr(sample, "SUMMARY_JOBS", {"alls-green"})
    jobs = [_job(200, "alls-green", "failed"), _job(201, "test", "failed")]
    payload = sample.build_report(jobs, ENV)
    by_name = {j["name"]: j for j in payload["jobs"]}
    assert by_name["alls-green"]["summary"] is True
    assert by_name["test"]["summary"] is False
    report = PipelineReportIn.model_validate(payload)
    flagged = next(j for j in report.jobs if j.name == "alls-green")
    assert flagged.summary is True


def test_payload_validates_against_the_real_schema():
    report = PipelineReportIn.model_validate(sample.build_report(PIPELINE_JOBS, ENV))
    assert report.provider == "gitlab"
    assert report.ci_run_attempt == 1
    assert len(report.jobs) == 6


def test_a_job_only_reaches_the_report_once():
    # map_job on the retried pair is what PipelineReportIn's uniqueness needs.
    payload = sample.build_report(PIPELINE_JOBS, ENV)
    ids = [j["ci_job_id"] for j in payload["jobs"]]
    assert len(ids) == len(set(ids))


def test_unmapped_statuses_produce_no_jobs():
    running_only = [_job(1, "a", "running"), _job(2, "b", "scheduled"), _job(3, "c", "pending")]
    payload = sample.build_report(running_only, ENV)
    # Nothing to post: PipelineReportIn requires at least one job.
    assert payload["jobs"] == []
    with pytest.raises(ValidationError):
        PipelineReportIn.model_validate(payload)


def test_the_reporter_job_is_skipped_even_when_it_is_final():
    # CI_JOB_ID is the reporter's own job; it never reports itself.
    jobs = [_job(112, "flakeradar:report", "success"), _job(100, "build", "success")]
    payload = sample.build_report(jobs, ENV)
    assert [j["ci_job_id"] for j in payload["jobs"]] == ["100"]


def test_map_job_returns_none_for_unmapped_status():
    assert sample.map_job(_job(7, "wip", "running")) is None
    assert sample.map_job(_job(7, "wip", "skipped"))["status"] == "skipped"
