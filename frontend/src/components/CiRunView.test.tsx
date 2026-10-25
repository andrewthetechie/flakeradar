import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CiRunFailedJob, CiRunFailures, CiRunTestFailure } from "../api";
import { CiRunView } from "./CiRunView";

function failure(over: Partial<CiRunTestFailure> = {}): CiRunTestFailure {
  return {
    test_id: 1,
    project: "backend",
    suite: "",
    classname: "tests.test_login",
    name: "test_login",
    run_id: 11,
    ci_job_id: "9.1",
    status: "failed",
    failed_attempts: 1,
    failure_category: "network",
    message: "Connection refused",
    tier: "flaky",
    flakiness_score: 0.6,
    confirmed_flake_count: 2,
    clean_streak: 0,
    quarantined: false,
    github_issue_number: 12,
    github_issue_url: "https://github.com/a/b/issues/12",
    ...over,
  };
}

function failedJob(over: Partial<CiRunFailedJob> = {}): CiRunFailedJob {
  return {
    job_id: 5,
    pipeline: ".github/workflows/ci.yml",
    name: "test (ubuntu)",
    ci_job_id: "9.1",
    url: "https://github.com/a/b/actions/runs/99/job/5",
    outcome: "unexplained",
    summary: false,
    tier: "suspect",
    flakiness_score: 0.4,
    confirmed_flake_count: 1,
    clean_streak: 0,
    github_issue_number: null,
    github_issue_url: null,
    ...over,
  };
}

function body(over: Partial<CiRunFailures> = {}): CiRunFailures {
  return {
    repo: "a/b",
    ci_run_id: "99",
    ci_run_attempt: 2,
    flake_threshold: 0.3,
    reports: { pending: 0, processed: 2, failed: 0 },
    total_failures: 1,
    truncated: false,
    failures: [failure()],
    failed_jobs: [failedJob()],
    ...over,
  };
}

/** Stub fetch for /api/ci-runs/… and record every URL it is called with. */
function stubFetch(make: () => Response): string[] {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      urls.push(url);
      return Promise.resolve(make());
    }),
  );
  return urls;
}

const json = (value: unknown) => new Response(JSON.stringify(value));

/** CiRunView takes the attempt from the URL state, so tests need a parent that holds it. */
function Harness({ start = null }: { start?: number | null }) {
  const [attempt, setAttempt] = useState<number | null>(start);
  return (
    <CiRunView
      repo="a/b"
      ciRun="99"
      attempt={attempt}
      onAttempt={setAttempt}
      onOpenTest={() => {}}
      onOpenJob={() => {}}
    />
  );
}

/** Let queued timers and promise chains run (fake timers make findByText unusable). */
async function flush() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("CiRunView", () => {
  it("says reports are still processing and asks again after 5 s", async () => {
    vi.useFakeTimers();
    const urls = stubFetch(() => json(body({ reports: { pending: 2, processed: 1, failed: 0 } })));
    render(<Harness />);
    await flush();

    expect(screen.getByText("Still processing 2 reports")).toBeInTheDocument();
    expect(urls.filter((u) => u.startsWith("/api/ci-runs/"))).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(urls.filter((u) => u.startsWith("/api/ci-runs/"))).toHaveLength(2);
    expect(urls.at(-1)).toContain("/api/ci-runs/99/failures");
  });

  it("stops polling once every report is processed", async () => {
    vi.useFakeTimers();
    const urls = stubFetch(() => json(body()));
    render(<Harness />);
    await flush();

    expect(screen.queryByText(/Still processing/)).not.toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30000);
    });
    expect(urls.filter((u) => u.startsWith("/api/ci-runs/"))).toHaveLength(1);
  });

  it("keeps asking every 5 s for as long as reports are pending", async () => {
    vi.useFakeTimers();
    const urls = stubFetch(() => json(body({ reports: { pending: 1, processed: 1, failed: 0 } })));
    render(<Harness />);
    await flush();

    // One act per round: the poll is armed from the answer, so React has to
    // re-render before the next timer exists.
    for (let round = 0; round < 3; round++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
    }
    // t=0, 5 s, 10 s, 15 s
    expect(urls.filter((u) => u.startsWith("/api/ci-runs/"))).toHaveLength(4);
  });

  it("warns when a report of the CI run failed to process", async () => {
    stubFetch(() => json(body({ reports: { pending: 0, processed: 1, failed: 1 } })));
    render(<Harness />);
    expect(await screen.findByText(/1 report failed to process/)).toBeInTheDocument();
  });

  it("lists the failing tests and the failed jobs of a complete run", async () => {
    const onOpenTest = vi.fn();
    const onOpenJob = vi.fn();
    stubFetch(() =>
      json(
        body({
          total_failures: 2,
          failures: [
            failure({ status: "passed", failed_attempts: 2 }),
            failure({
              test_id: 2,
              name: "test_upload",
              classname: "tests.test_upload",
              status: "failed",
              failure_category: null,
              tier: "stable",
              flakiness_score: 0.05,
              confirmed_flake_count: 0,
              quarantined: true,
              github_issue_number: null,
              github_issue_url: null,
            }),
          ],
          failed_jobs: [failedJob({ summary: true })],
        }),
      ),
    );
    render(
      <CiRunView
        repo="a/b"
        ciRun="99"
        attempt={null}
        onAttempt={() => {}}
        onOpenTest={onOpenTest}
        onOpenJob={onOpenJob}
      />,
    );

    expect(await screen.findByText("passed on retry")).toBeInTheDocument();
    expect(screen.getByText("tests.test_login")).toBeInTheDocument();
    expect(screen.getByText("likely: network")).toBeInTheDocument();
    expect(screen.getByText("0.60")).toBeInTheDocument();
    expect(screen.getByText("flaky")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "#12" })).toHaveAttribute(
      "href",
      "https://github.com/a/b/issues/12",
    );
    // The second row is still failing, and quarantined; only a retry is renamed.
    expect(screen.getByRole("button", { name: "test_upload" })).toBeInTheDocument();
    expect(screen.getByText("failed")).toBeInTheDocument();
    expect(screen.getByText("⏻ quarantined")).toBeInTheDocument();
    expect(screen.getAllByText("backend")).toHaveLength(2);

    expect(screen.getByText("summary")).toBeInTheDocument();
    expect(screen.getByText("unexplained")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "CI job" })).toHaveAttribute(
      "href",
      "https://github.com/a/b/actions/runs/99/job/5",
    );

    fireEvent.click(screen.getByRole("button", { name: /test_login/ }));
    expect(onOpenTest).toHaveBeenCalledWith(1);
    fireEvent.click(screen.getByRole("button", { name: /test \(ubuntu\)/ }));
    expect(onOpenJob).toHaveBeenCalledWith(5);
  });

  it("says how many failures are shown when the response is truncated", async () => {
    stubFetch(() =>
      json(
        body({
          truncated: true,
          total_failures: 800,
          failures: Array.from({ length: 500 }, (_, i) => failure({ test_id: i + 1 })),
        }),
      ),
    );
    render(<Harness />);
    // The server's total, not the length of the capped list.
    expect(await screen.findByText("Showing 500 of 800")).toBeInTheDocument();
  });

  it("says so when FlakeRadar has no reports for the CI run", async () => {
    stubFetch(() => new Response("not found", { status: 404 }));
    render(<Harness start={3} />);
    expect(
      await screen.findByText("FlakeRadar has no reports for this CI run"),
    ).toBeInTheDocument();
  });

  it("re-requests the attempt picked in the number input", async () => {
    const urls = stubFetch(() => json(body()));
    render(<Harness />);
    expect(await screen.findByRole("button", { name: "test_login" })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Attempt"), { target: { value: "3" } });
    await waitFor(() => expect(urls.at(-1)).toContain("ci_run_attempt=3"));
    expect(screen.getByLabelText("Attempt")).toHaveValue(3);
  });
});
