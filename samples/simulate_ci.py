"""Replay a realistic CI history into a running FlakeRadar instance.

Usage:  python samples/simulate_ci.py [base_url] [token]
Defaults: http://localhost:8000  /  changeme

Simulates 14 CI runs of one Repo with two Projects:
- demo/shop : backend
    test_checkout_total_rounding  -> genuinely flaky (random failures + one
                                     same-SHA retry that flips fail->pass)
    test_payment_gateway_timeout  -> mildly flaky (occasional failure)
    test_schema_migration_v42     -> broken: fails EVERY run (scores 0)
    5 stable tests                -> always pass
- demo/shop : frontend
    Cart > updates the badge      -> flaky, reports its file (Location demo)
Uploads are queued (202); the script waits until every Report is processed.
"""
import random
import sys
import time

import httpx

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8000"
TOKEN = sys.argv[2] if len(sys.argv) > 2 else "changeme"
REPO = "demo/shop"

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


def report(suite: str, cases: list[str]) -> bytes:
    return (f'<testsuites><testsuite name="{suite}" tests="{len(cases)}">'
            f'{"".join(cases)}</testsuite></testsuites>').encode()


def post(client: httpx.Client, project: str, root: str, sha: str, run_id: str,
         body: bytes) -> int:
    resp = client.post(
        f"{BASE}/api/ingest",
        params={"repo": REPO, "project": project, "root": root, "commit_sha": sha,
                "branch": "main", "ci_run_id": run_id},
        content=body,
        headers={"X-API-Key": TOKEN, "Content-Type": "application/xml"},
    )
    resp.raise_for_status()
    return resp.json()["report_id"]


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
            report_ids.append(post(client, "backend", "", sha, f"run-{i}",
                                   report("backend", backend_cases(checkout, gateway))))
            badge = "failed" if rng.random() < 0.3 else "passed"
            report_ids.append(post(client, "frontend", "web", sha, f"run-{i}", report("frontend", [
                case_xml("Cart > updates the badge", badge, classname="src/Cart.test.tsx",
                         file="src/Cart.test.tsx", line=21),
            ])))

            # The habit FlakeRadar exploits: a failed run gets re-run on the
            # same commit. Replay run 6's failure as a same-SHA retry that passes.
            if i == 6:
                report_ids.append(post(client, "backend", "", sha, f"run-{i}-retry",
                                       report("backend", backend_cases("passed", gateway))))

        deadline = time.monotonic() + 60
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
