#!/usr/bin/env python3
"""Report this GitLab Pipeline's Jobs to FlakeRadar as one Pipeline report.

Standard library only, so it runs in a plain ``python:3.12-alpine`` job image.
Copy this file into your repository (e.g. ``ci/flakeradar/report_pipeline.py``)
and run it from a ``stage: .post`` job with ``when: always``; see
``samples/gitlab/gitlab-ci.yml`` next to it.

Required environment:
  FLAKERADAR_URL    base URL of your FlakeRadar, e.g. https://flakeradar.example.com
  FLAKERADAR_TOKEN  your FlakeRadar API token (FLAKERADAR_API_TOKEN), sent as X-API-Key
  GITLAB_TOKEN      project access token, Reporter role with the read_api scope.
                    CI_JOB_TOKEN does not work here: in the Jobs API a job token may
                    call only ``GET /job``, so it cannot list a pipeline's jobs.

A GitLab retry is a NEW job id in the SAME pipeline, so one report carries both the
failed try and the successful retry of a Job: a fail and a pass on one commit SHA,
which is a proven flake. ``ci_run_attempt`` is therefore always 1.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

# Maps a final GitLab job status to FlakeRadar's JobStatus ("passed" | "failed" | "skipped").
# failed maps to "failed" even when allow_failure is true (mapping is status-only;
# do not filter or special-case allow_failure).
STATUS_MAP = {
    "success": "passed",
    "failed": "failed",
    "canceled": "skipped",
    "skipped": "skipped",
    "manual": "skipped",
}


def map_job(job: dict) -> dict | None:
    """One GitLab API job object -> one JobResultIn-shaped dict.

    Returns None when the status has no mapping (running, pending, created, scheduled,
    preparing, canceling, waiting_for_resource, waiting_for_callback). Unmapped jobs are
    left out -- which is how the reporter job itself (still ``running`` at ``.post``
    time) is excluded.
    """
    status = STATUS_MAP.get(job.get("status") or "")
    if status is None:
        return None
    return {
        "ci_job_id": str(job["id"]),
        "name": job["name"],
        "status": status,
        "url": job.get("web_url") or "",
        "runner_name": (job.get("runner") or {}).get("description") or "",
        "runner_labels": list(job.get("tag_list") or []),
        "started_at": job.get("started_at"),
        "completed_at": job.get("finished_at"),
    }


def build_report(jobs: list[dict], env: dict[str, str]) -> dict:
    """GitLab API job objects + the CI environment -> a PipelineReportIn payload.

    Callers must not post the result when ``jobs`` is empty: the schema requires at
    least one Job.
    """
    results = []
    for job in jobs:
        if str(job.get("id")) == env.get("CI_JOB_ID"):
            continue  # the reporter job never reports itself
        mapped = map_job(job)
        if mapped is not None:
            results.append(mapped)
    return {
        "repo": env["CI_PROJECT_PATH"],
        "provider": "gitlab",
        "pipeline": ".gitlab-ci.yml",
        "commit_sha": env["CI_COMMIT_SHA"],
        "branch": env["CI_COMMIT_REF_NAME"],
        "default_branch": env["CI_DEFAULT_BRANCH"],
        "ci_run_id": env["CI_PIPELINE_ID"],
        "ci_run_attempt": 1,  # a GitLab retry is a new job id, not a new attempt
        "jobs": results,
    }


def fetch_jobs(api_base: str, project_id: str, pipeline_id: str, token: str) -> list[dict]:
    """Every job of the pipeline, retried ones included, following X-Next-Page."""
    jobs: list[dict] = []
    page = "1"
    while page:
        url = (
            f"{api_base}/projects/{project_id}/pipelines/{pipeline_id}/jobs"
            f"?include_retried=true&per_page=100&page={page}"
        )
        request = urllib.request.Request(url, headers={"PRIVATE-TOKEN": token})
        try:
            with urllib.request.urlopen(request, timeout=30) as resp:
                jobs.extend(json.loads(resp.read()))
                page = resp.headers.get("X-Next-Page") or ""
        except urllib.error.HTTPError as exc:
            sys.exit(f"GitLab API error {exc.code}: {exc.read().decode(errors='replace')}")
        except urllib.error.URLError as exc:
            sys.exit(f"GitLab API unreachable: {exc.reason}")
    return jobs


def post_report(base_url: str, api_token: str, payload: dict, attempts: int = 5) -> None:
    """Queue one Pipeline report, retrying 5xx responses like the GitHub sample."""
    body = json.dumps(payload).encode()
    last_error = ""
    for i in range(1, attempts + 1):
        request = urllib.request.Request(
            f"{base_url}/api/ingest/pipeline",
            data=body,
            headers={"Content-Type": "application/json", "X-API-Key": api_token},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as resp:
                print(f"Queued report {json.loads(resp.read()).get('report_id')}")
                return
        except urllib.error.HTTPError as exc:
            last_error = f"{exc.code} {exc.read().decode(errors='replace')}"
            if exc.code < 500:  # a 4xx will not fix itself; fail fast
                sys.exit(f"FlakeRadar rejected the report: {last_error}")
        except urllib.error.URLError as exc:
            last_error = str(exc.reason)
        if i < attempts:
            time.sleep(2 * i)
    sys.exit(f"FlakeRadar unreachable after {attempts} attempts: {last_error}")


def main() -> None:
    missing = [k for k in ("FLAKERADAR_URL", "FLAKERADAR_TOKEN", "GITLAB_TOKEN") if not os.environ.get(k)]
    if missing:
        sys.exit(f"Missing required environment variable(s): {', '.join(missing)}")
    env = dict(os.environ)
    jobs = fetch_jobs(env["CI_API_V4_URL"], env["CI_PROJECT_ID"], env["CI_PIPELINE_ID"], env["GITLAB_TOKEN"])
    report = build_report(jobs, env)
    if not report["jobs"]:
        print("No reportable jobs.")
        return
    post_report(env["FLAKERADAR_URL"].rstrip("/"), env["FLAKERADAR_TOKEN"], report)


if __name__ == "__main__":
    main()
