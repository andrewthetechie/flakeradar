"""Replay a realistic CI history into a running FlakeRadar instance.

Usage:  python samples/simulate_ci.py [base_url] [token]
Defaults: http://localhost:8000  /  changeme

Simulates 14 CI runs of one Repo with two Projects and a CI pipeline:
- demo/shop : backend
    test_checkout_total_rounding  -> genuinely flaky (random failures + one
                                     same-SHA retry that flips fail->pass)
    test_payment_gateway_timeout  -> mildly flaky (occasional failure)
    test_schema_migration_v42     -> broken: fails EVERY run (scores 0)
    5 stable tests                -> always pass
- demo/shop : frontend
    Cart > updates the badge      -> flaky, reports its file (Location demo)
    Checkout > shows the total   -> passes, but needs a retry every 4th run (flakyFailure)
- pipeline .github/workflows/ci.yml with Jobs backend, frontend, lint, and
  "e2e (ubuntu-latest)" — an infrastructure flake that fails ~20% of the time
  with NO failing Tests (plus one same-SHA re-run attempt that passes).

The JUnit reports carry the matching ci_job_id, so backend/frontend failures
EXPLAIN those Jobs' failures; e2e has no JUnit, so its failures are
unexplained and it should score higher (flakier) than backend.

Reports are queued (202); the script waits until every Report is processed.
"""
import random
import sys
import time

import httpx

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8000"
TOKEN = sys.argv[2] if len(sys.argv) > 2 else "changeme"
REPO = "demo/shop"
PIPELINE = ".github/workflows/ci.yml"

rng = random.Random(42)

STABLE = [
    "test_login_with_valid_credentials",
    "test_signup_sends_welcome_email",
    "test_cart_add_and_remove",
    "test_product_search_pagination",
    "test_invoice_pdf_render",
]


def case_xml(name: str, status: str, classname: str = "tests.e2e.test_shop",
             file: str = "tests/e2e/test_shop.py", line: int = 88) -> str:
    body = ""
    if status == "failed":
        body = (
            '<failure message="AssertionError: expected 104.85, got 104.84">'
            f"Traceback (most recent call last):\n  File \"{file}\", line {line}, in {name}\n"
            "    assert total == Decimal('104.85')\n"
            "AssertionError: expected 104.85, got 104.84</failure>"
        )
    return (f'<testcase classname="{classname}" name="{name}" file="{file}" line="{line}" '
            f'time="{rng.uniform(0.1, 2.5):.2f}">{body}</testcase>')


def retried_case_xml(name: str, classname: str, file: str, line: int) -> str:
    """A Playwright-style test that failed once and passed on retry (includeRetries)."""
    return (
        f'<testcase classname="{classname}" name="{name}" file="{file}" line="{line}" time="1.40">'
        '<flakyFailure message="Timed out 5000ms waiting for expect(locator).toHaveText(expected)" '
        'type="FAILURE" time="5.10"><stackTrace>Error: Timed out 5000ms waiting for '
        f"expect(locator).toHaveText(expected)\n    at " + file + ":" + str(line) + "</stackTrace>"
        "</flakyFailure></testcase>"
    )


def report(suite: str, cases: list[str]) -> bytes:
    return (f'<testsuites><testsuite name="{suite}" tests="{len(cases)}">'
            f'{"".join(cases)}</testsuite></testsuites>').encode()


def post_junit(client: httpx.Client, project: str, root: str, sha: str, run_id: str,
               ci_job_id: str, ci_run_attempt: int, body: bytes) -> int:
    resp = client.post(
        f"{BASE}/api/ingest",
        params={"repo": REPO, "project": project, "root": root, "commit_sha": sha,
                "branch": "main", "ci_run_id": run_id, "ci_run_attempt": ci_run_attempt,
                "ci_job_id": ci_job_id, "pipeline": PIPELINE, "default_branch": "main"},
        content=body,
        headers={"X-API-Key": TOKEN, "Content-Type": "application/xml"},
    )
    resp.raise_for_status()
    return resp.json()["report_id"]


def post_pipeline(client: httpx.Client, sha: str, run_id: str, ci_run_attempt: int,
                  jobs: list[dict]) -> int:
    resp = client.post(
        f"{BASE}/api/ingest/pipeline",
        json={
            "repo": REPO, "provider": "github", "pipeline": PIPELINE,
            "commit_sha": sha, "branch": "main", "default_branch": "main",
            "ci_run_id": run_id, "ci_run_attempt": ci_run_attempt,
            "jobs": jobs,
        },
        headers={"X-API-Key": TOKEN, "Content-Type": "application/json"},
    )
    resp.raise_for_status()
    return resp.json()["report_id"]


def job(ci_job_id: str, name: str, status: str) -> dict:
    return {
        "ci_job_id": ci_job_id, "name": name, "status": status,
        "url": f"{BASE}/job/{ci_job_id}", "runner_name": "ubuntu-latest",
        "runner_labels": ["ubuntu-latest", "self-hosted"],
    }


def backend_cases(checkout: str, gateway: str) -> list[str]:
    cases = [case_xml(n, "passed") for n in STABLE]
    cases.append(case_xml("test_checkout_total_rounding", checkout))
    cases.append(case_xml("test_payment_gateway_timeout", gateway))
    cases.append(case_xml("test_schema_migration_v42", "failed"))
    return cases


def main():
    report_ids: list[int] = []
    with httpx.Client(timeout=10) as client:
        for i in range(14):
            sha = f"{rng.getrandbits(160):040x}"
            checkout = "failed" if rng.random() < 0.35 else "passed"
            if i == 6:
                checkout = "failed"  # guarantee the same-SHA retry demo below
            gateway = "failed" if rng.random() < 0.15 else "passed"
            badge = "failed" if rng.random() < 0.3 else "passed"

            # Jobs in this run's Pipeline (attempt 1).
            backend_status = "failed" if checkout == "failed" else "passed"
            frontend_status = "failed" if badge == "failed" else "passed"
            e2e_status = "failed" if rng.random() < 0.20 else "passed"

            # JUnit reports linked to their Job executions for attribution.
            report_ids.append(post_junit(client, "backend", "", sha, f"run-{i}", f"b-{i}", 1,
                                         report("backend", backend_cases(checkout, gateway))))
            report_ids.append(post_junit(client, "frontend", "web", sha, f"run-{i}", f"f-{i}", 1,
                                         report("frontend", [
                                             case_xml("Cart > updates the badge", badge,
                                                      classname="src/Cart.test.tsx",
                                                      file="src/Cart.test.tsx", line=21),
                                             (retried_case_xml("Checkout > shows the total", "web/src/checkout.spec.ts",
                                                               "src/checkout.spec.ts", 21)
                                              if i % 4 == 0
                                              else case_xml("Checkout > shows the total", "passed",
                                                            classname="web/src/checkout.spec.ts",
                                                            file="src/checkout.spec.ts", line=21)),
                                         ])))
            # The Pipeline report: backend + frontend + lint + the e2e infra flake.
            report_ids.append(post_pipeline(client, sha, f"run-{i}", 1, [
                job(f"b-{i}", "backend", backend_status),
                job(f"f-{i}", "frontend", frontend_status),
                job(f"l-{i}", "lint", "passed"),
                job(f"e-{i}", "e2e (ubuntu-latest)", e2e_status),
            ]))

            # The habit FlakeRadar exploits: a failed run gets re-run on the
            # same commit. Replay run 6's backend failure as a same-SHA retry
            # that passes, and its infra e2e failure as an attempt-2 pass.
            if i == 6:
                report_ids.append(post_junit(client, "backend", "", sha, "run-6-retry", f"b-6-2", 2,
                                             report("backend", backend_cases("passed", gateway))))
                report_ids.append(post_pipeline(client, sha, "run-6-retry", 2, [
                    job(f"b-6-2", "backend", "passed"),
                    job(f"e-6-2", "e2e (ubuntu-latest)", "passed"),
                ]))

        deadline = time.monotonic() + 120
        while True:
            summary = client.get(f"{BASE}/api/reports/summary").json()
            if summary["pending"] == 0:
                break
            if time.monotonic() > deadline:
                sys.exit(f"timed out waiting for {summary['pending']} pending reports")
            time.sleep(0.5)

    print(f"uploaded {len(report_ids)} reports; failed to process: {summary['failed']}")
    print(f"open {BASE}/?repo={REPO}")


if __name__ == "__main__":
    main()
