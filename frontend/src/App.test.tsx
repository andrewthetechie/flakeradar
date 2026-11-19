import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { History, Job, JobHistory, TestCase, TestPage } from "./api";
import App from "./App";

function page(name: string): TestPage {
  const test: TestCase = {
    id: name.length,
    repo: "x/y",
    project: "default",
    fingerprint: name,
    suite: "",
    classname: "c",
    name,
    file: null,
    line: null,
    flakiness_score: 0.5,
    tier: "flaky",
    confirmed_flake_count: 0,
    failure_category: null,
    clean_streak: 0,
    fail_streak: 0,
    broken: false,
    trend: null,
    last_status: "failed",
    last_seen_at: "2026-09-25T00:00:00Z",
    quarantined: false,
    quarantined_at: null,
    quarantine_reason: null,
    quarantine_expires_at: null,
    acknowledged_until: null,
    acknowledge_reason: null,
    github_issue_number: null,
    github_issue_url: null,
    github_issue_closed: false,
    impacted_runs: 0,
    impacted_branches: 0,
    time_lost_seconds: 0,
  };
  return { items: [test], total: 1, page: 1, page_size: 50 };
}

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body)));

const testsSummary = {
  total_tests: 1,
  flaky_tests: 1,
  suspect_tests: 0,
  confirmed_flaky_tests: 0,
  total_runs: 1,
  executions_30d: 1,
  failures_30d: 0,
  flake_threshold: 0.3,
  new_flaky_7d: 0,
  category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
};

/** Stub fetch by URL prefix, checked in order (put /history before its list), and record every URL. */
function stubOrdered(
  routes: Array<[prefix: string, reply: () => Response | Promise<Response>]>,
): string[] {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      urls.push(url);
      for (const [prefix, reply] of routes) if (url.startsWith(prefix)) return reply();
      return json({});
    }),
  );
  return urls;
}

/** A Test drawer's history for the requested test id (name "renders"), from TestDrawer.test.tsx. */
function testHistory(id: number, overrides: Partial<History> = {}): History {
  return {
    test: {
      id,
      repo: "andrewthetechie/writers-app",
      project: "frontend",
      fingerprint: "f",
      suite: "unit",
      classname: "src/app.test.ts",
      name: "renders",
      file: "src/app.test.ts",
      line: 12,
      flakiness_score: 0.6,
      tier: "flaky",
      confirmed_flake_count: 1,
      failure_category: "assertion",
      clean_streak: 3,
      fail_streak: 0,
      broken: false,
      trend: "steady",
      last_status: "passed",
      last_seen_at: "2026-09-25T00:00:00Z",
      quarantined: false,
      quarantined_at: null,
      quarantine_reason: null,
      quarantine_expires_at: null,
      acknowledged_until: null,
      acknowledge_reason: null,
      github_issue_number: null,
      github_issue_url: null,
      github_issue_closed: false,
      impacted_runs: 0,
      impacted_branches: 0,
      time_lost_seconds: 0,
    },
    location: {
      path: "frontend/src/app.test.ts",
      line: 12,
      url: "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12",
    },
    last_failing_sha: "aaa111",
    last_failing_branch: "main",
    jobs: [],
    executions: [],
    score_history: [],
    ...overrides,
  };
}

const jobPageFixture = {
  items: [
    {
      id: 3,
      repo: "x/y",
      provider: "github",
      pipeline: ".github/workflows/ci.yml",
      name: "e2e",
      flakiness_score: 0.31,
      tier: "flaky",
      confirmed_flake_count: 0,
      clean_streak: 0,
      fail_streak: 0,
      broken: false,
      trend: null,
      last_status: "failed",
      last_seen_at: "2026-09-25T00:00:00Z",
      github_issue_number: null,
      github_issue_url: null,
      github_issue_closed: false,
    },
  ],
  total: 1,
  page: 1,
  page_size: 50,
};

const jobSummaryFixture = {
  total_jobs: 1,
  flaky_jobs: 1,
  suspect_jobs: 0,
  confirmed_flaky_jobs: 0,
  job_executions_30d: 1,
  job_failures_30d: 0,
  flake_threshold: 0.3,
  new_flaky_7d: 0,
  pipelines: [],
};

/** A Job drawer's history, from JobDrawer.test.tsx (its Job is id 3, explaining Test id 9). */
function jobHistory(overrides: Partial<JobHistory> = {}): JobHistory {
  return {
    job: {
      id: 3,
      repo: "acme/app",
      provider: "github",
      pipeline: ".github/workflows/ci.yml",
      name: "e2e (ubuntu-latest)",
      flakiness_score: 0.31,
      tier: "flaky",
      confirmed_flake_count: 0,
      clean_streak: 4,
      trend: null,
      last_status: "failed",
      last_seen_at: "2026-09-25T00:00:00Z",
      summary: false,
      acknowledged_until: null,
      acknowledge_reason: null,
      github_issue_number: null,
      github_issue_url: null,
      github_issue_closed: false,
      impacted_runs: 0,
      impacted_branches: 0,
      time_lost_seconds: 0,
    },
    unexplained_failures: 1,
    explained_failures: 1,
    executions: [
      {
        id: 2,
        status: "failed",
        outcome: "explained",
        commit_sha: "bbb222",
        branch: "feat",
        ci_run_id: "2",
        ci_run_attempt: 1,
        ci_job_id: "J2",
        url: "https://github.com/acme/app/actions/runs/2/jobs/J2",
        runner_name: "runner-1",
        runner_labels: ["ubuntu-latest", "self-hosted"],
        started_at: "2026-09-25T00:00:00Z",
        completed_at: "2026-09-25T00:01:00Z",
        created_at: "2026-09-25T00:01:00Z",
        explained_by: [
          {
            test_id: 9,
            project: "backend",
            classname: "tests",
            name: "test_login",
            status: "failed",
          },
        ],
        explained_by_jobs: [],
      },
    ],
    score_history: [],
    runners: [],
    ...overrides,
  };
}

/** A complete CI run answer, from CiRunView.test.tsx (failure test_id 1, failed job 5). */
function ciRunBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    repo: "a/b",
    ci_run_id: "99",
    ci_run_attempt: 2,
    flake_threshold: 0.3,
    reports: { pending: 0, processed: 2, failed: 0 },
    total_failures: 1,
    truncated: false,
    failures: [
      {
        test_id: 1,
        project: "backend",
        suite: "",
        classname: "tests.test_login",
        name: "test_login",
        run_id: 11,
        ci_job_id: "9.1",
        infra_suspect: false,
        status: "failed",
        failed_attempts: 1,
        failure_category: "network",
        message: "Connection refused",
        tier: "flaky",
        flakiness_score: 0.6,
        confirmed_flake_count: 2,
        clean_streak: 0,
        fail_streak: 0,
        broken: false,
        quarantined: false,
        github_issue_number: null,
        github_issue_url: null,
        github_issue_closed: false,
      },
    ],
    failed_jobs: [
      {
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
        fail_streak: 0,
        broken: false,
        github_issue_number: null,
        github_issue_url: null,
        github_issue_closed: false,
      },
    ],
    ...overrides,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

describe("App", () => {
  it("ignores a slow response for a scope the user already left", async () => {
    let finishOld: (r: Response) => void = () => {};
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/tests") && url.includes("repo=a%2Fone")) {
          return new Promise<Response>((resolve) => {
            finishOld = resolve;
          });
        }
        if (url.startsWith("/api/tests")) return json(page("newtest"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    window.history.replaceState(null, "", "/?repo=a%2Fone");
    render(<App />);

    // Browser Forward to another repo while the first request is still in flight.
    act(() => {
      window.history.pushState(null, "", "/?repo=b%2Ftwo");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(await screen.findByText("newtest")).toBeInTheDocument();

    await act(async () => {
      finishOld(new Response(JSON.stringify(page("oldtest"))));
    });
    expect(screen.queryByText("oldtest")).not.toBeInTheDocument();
    expect(screen.getByText("newtest")).toBeInTheDocument();
  });

  it("loads the Jobs view when ?view=jobs is set", async () => {
    const jobPage = {
      items: [
        {
          id: 3,
          repo: "x/y",
          provider: "github",
          pipeline: ".github/workflows/ci.yml",
          name: "e2e",
          flakiness_score: 0.31,
          tier: "flaky",
          confirmed_flake_count: 0,
          clean_streak: 0,
          fail_streak: 0,
          broken: false,
          trend: null,
          last_status: "failed",
          last_seen_at: "2026-09-25T00:00:00Z",
          github_issue_number: null,
          github_issue_url: null,
          github_issue_closed: false,
        },
      ],
      total: 1,
      page: 1,
      page_size: 50,
    };
    const jobSummary = {
      total_jobs: 1,
      flaky_jobs: 1,
      suspect_jobs: 0,
      confirmed_flaky_jobs: 0,
      job_executions_30d: 10,
      job_failures_30d: 0,
      flake_threshold: 0.3,
      new_flaky_7d: 0,
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/jobs/summary")) return json(jobSummary);
        if (url.startsWith("/api/jobs")) return json(jobPage);
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs");
    render(<App />);
    expect(await screen.findByText("e2e")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "jobs" })).toHaveAttribute("aria-pressed", "true");
    // The Jobs view gets its own search box, wired to the same q.
    expect(screen.getByPlaceholderText("Job or pipeline")).toBeInTheDocument();
    // The Tests view's quarantined toggle stays off the Jobs view; the Issue
    // filter and the Pipeline filter are both wired here.
    expect(screen.queryByRole("checkbox", { name: /quarantined only/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /issue/i })).toBeInTheDocument();
    // This summary carries no pipelines, so the Pipeline select lists only All.
    expect(
      within(screen.getByRole("combobox", { name: /pipeline/i })).getAllByRole("option"),
    ).toHaveLength(1);
  });

  it("renders the CI run view instead of the leaderboard for ?view=ci-run", async () => {
    const ciRun = {
      repo: "a/b",
      ci_run_id: "99",
      ci_run_attempt: 1,
      flake_threshold: 0.3,
      reports: { pending: 0, processed: 2, failed: 0 },
      total_failures: 1,
      truncated: false,
      failures: [
        {
          test_id: 4,
          project: "backend",
          suite: "",
          classname: "tests.test_login",
          name: "login_page",
          run_id: 8,
          ci_job_id: "99.1",
          infra_suspect: false,
          status: "passed",
          failed_attempts: 1,
          failure_category: null,
          message: "Connection refused",
          tier: "flaky",
          flakiness_score: 0.5,
          confirmed_flake_count: 1,
          clean_streak: 0,
          fail_streak: 0,
          broken: false,
          quarantined: false,
          github_issue_number: null,
          github_issue_url: null,
          github_issue_closed: false,
        },
      ],
      failed_jobs: [],
    };
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/ci-runs/")) return json(ciRun);
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=ci-run&repo=a%2Fb&ci_run=99");
    render(<App />);

    expect(await screen.findByText("passed on retry")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "login_page" })).toBeInTheDocument();
    expect(urls.some((u) => u.startsWith("/api/ci-runs/99/failures?repo=a%2Fb"))).toBe(true);
    // The CI run view replaces the leaderboard and must not query it.
    expect(screen.queryByText("Flakiness leaderboard")).not.toBeInTheDocument();
    expect(urls.filter((u) => u.startsWith("/api/tests") || u.startsWith("/api/summary"))).toEqual(
      [],
    );
  });

  it("shows the URL hint when the ci-run view is opened with no run selected", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=ci-run");
    render(<App />);

    expect(await screen.findByText(/No CI run selected/)).toBeInTheDocument();
    // The hint names the exact query the developer must add.
    expect(screen.getByText(/\?view=ci-run&repo=owner\/name&ci_run=12345678/)).toBeInTheDocument();
    // No leaderboard and no run fetch: the view is inert until a run is given.
    expect(screen.queryByText("Flakiness leaderboard")).not.toBeInTheDocument();
  });

  it("opens the CI run view from the header input", async () => {
    const ciRun = {
      repo: "x/y",
      ci_run_id: "4242",
      ci_run_attempt: 1,
      flake_threshold: 0.3,
      reports: { pending: 0, processed: 1, failed: 0 },
      total_failures: 0,
      truncated: false,
      failures: [],
      failed_jobs: [],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/ci-runs/")) return json(ciRun);
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    render(<App />);
    // With no repo selected there is nothing to open a run for.
    expect(screen.queryByLabelText("Open CI run")).toBeNull();

    act(() => {
      window.history.pushState(null, "", "/?repo=x%2Fy");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });

    const input = await screen.findByLabelText("Open CI run");
    fireEvent.change(input, { target: { value: "4242" } });
    await userEvent.click(screen.getByRole("button", { name: "Open" }));

    expect(window.location.search).toContain("repo=x%2Fy");
    expect(window.location.search).toContain("view=ci-run");
    expect(window.location.search).toContain("ci_run=4242");
    expect(await screen.findByText("No test failures yet.")).toBeInTheDocument();
  });

  it("opens the CI run view from a Job drawer row, attempt included in the URL", async () => {
    const jobHistory = {
      job: {
        id: 3,
        repo: "x/y",
        provider: "github",
        pipeline: ".github/workflows/ci.yml",
        name: "e2e",
        flakiness_score: 0.31,
        tier: "flaky",
        confirmed_flake_count: 0,
        clean_streak: 0,
        fail_streak: 0,
        broken: false,
        trend: null,
        last_status: "failed",
        last_seen_at: "2026-09-25T00:00:00Z",
        summary: false,
        github_issue_number: null,
        github_issue_url: null,
        github_issue_closed: false,
      },
      unexplained_failures: 1,
      explained_failures: 0,
      executions: [
        {
          id: 2,
          status: "failed",
          outcome: "failed",
          commit_sha: "bbb222",
          branch: "feat",
          ci_run_id: "4242",
          ci_run_attempt: 3,
          ci_job_id: "J2",
          url: "",
          runner_name: "runner-1",
          runner_labels: [],
          started_at: null,
          completed_at: null,
          created_at: "2026-09-25T00:01:00Z",
          explained_by: [],
          explained_by_jobs: [],
        },
      ],
      score_history: [],
      runners: [],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/jobs/3/history")) return json(jobHistory);
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 1,
            flaky_jobs: 1,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 1,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
          });
        if (url.startsWith("/api/jobs"))
          return json({ items: [], total: 0, page: 1, page_size: 50 });
        if (url.startsWith("/api/ci-runs/"))
          return json({
            repo: "x/y",
            ci_run_id: "4242",
            ci_run_attempt: 3,
            flake_threshold: 0.3,
            reports: { pending: 0, processed: 1, failed: 0 },
            total_failures: 0,
            truncated: false,
            failures: [],
            failed_jobs: [],
          });
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&repo=x%2Fy&job=3");
    render(<App />);

    await userEvent.click(await screen.findByRole("button", { name: "run 4242" }));

    expect(window.location.search).toContain("view=ci-run");
    expect(window.location.search).toContain("ci_run=4242");
    expect(window.location.search).toContain("attempt=3");
    expect(await screen.findByText(/CI run 4242/)).toBeInTheDocument();
  });

  it("shows a failed Job drawer fetch's own message in the error banner", async () => {
    let finishHistory: (r: Response) => void = () => {};
    const jobPage = {
      items: [
        {
          id: 3,
          repo: "x/y",
          provider: "github",
          pipeline: ".github/workflows/ci.yml",
          name: "e2e",
          flakiness_score: 0.31,
          tier: "flaky",
          confirmed_flake_count: 0,
          clean_streak: 0,
          fail_streak: 0,
          broken: false,
          trend: null,
          last_status: "failed",
          last_seen_at: "2026-09-25T00:00:00Z",
          github_issue_number: null,
          github_issue_url: null,
          github_issue_closed: false,
        },
      ],
      total: 1,
      page: 1,
      page_size: 50,
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/jobs/3/history")) {
          return new Promise<Response>((resolve) => {
            finishHistory = resolve;
          });
        }
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 1,
            flaky_jobs: 1,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 1,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
          });
        if (url.startsWith("/api/jobs")) return json(jobPage);
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&repo=x%2Fy&job=3");
    render(<App />);

    // The history answer is held back until the leaderboard has painted, so
    // the refresh's setError(null) cannot wipe the banner the drawer raises.
    expect(await screen.findByText("e2e")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await act(async () => {
      finishHistory(new Response("boom", { status: 500 }));
    });
    const banner = await screen.findByRole("alert");
    expect(banner).toHaveTextContent(/\/api\/jobs\/3\/history\?limit=60 -> 500/);
    expect(banner.textContent).not.toContain("Error: ");
  });

  /**
   * A Jobs view whose Job 3 history load is held back, so a test can reject it
   * after the leaderboard painted: the refresh's setError(null) would otherwise
   * wipe the banner the drawer raises.
   */
  function stubJobHistoryRejection() {
    let failHistory: (reason: unknown) => void = () => {};
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/jobs/3/history")) {
          return new Promise<Response>((_, reject) => {
            failHistory = reject;
          });
        }
        if (url.startsWith("/api/jobs/summary")) return json(jobSummaryFixture);
        if (url.startsWith("/api/jobs")) return json(jobPageFixture);
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&repo=x%2Fy&job=3");
    return { fail: (reason: unknown) => failHistory(reason) };
  }

  it("surfaces a non-Error rejection from a Job drawer history as text", async () => {
    const { fail } = stubJobHistoryRejection();
    render(<App />);
    expect(await screen.findByText("e2e")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await act(async () => {
      fail("backend down");
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("backend down");
  });

  it("surfaces a non-Error rejection from a Test drawer history as text", async () => {
    let failHistory: (reason: unknown) => void = () => {};
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/tests/5/history")) {
          return new Promise<Response>((_, reject) => {
            failHistory = reject;
          });
        }
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(testsSummary);
      }),
    );
    render(<App />);
    await userEvent.click(await screen.findByText("login"));
    await act(async () => {});
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await act(async () => {
      failHistory("backend down");
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("backend down");
  });

  it("sends the stored pipeline and issue filters to /api/jobs", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 1,
            flaky_jobs: 1,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 4,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
            pipelines: ["ci.yml", "nightly.yml"],
          });
        if (url.startsWith("/api/jobs"))
          return json({ items: [], total: 0, page: 1, page_size: 50 });
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&pipeline=nightly.yml&issue=none");
    render(<App />);

    expect(await screen.findByText(/No flaky or suspect jobs in this view/)).toBeInTheDocument();
    const jobsUrl = urls.filter((u) => u.startsWith("/api/jobs?")).at(-1);
    expect(jobsUrl).toContain("pipeline=nightly.yml");
    expect(jobsUrl).toContain("has_issue=false");
    // The Pipeline select is filled from the summary, with the URL's value chosen.
    const select = screen.getByRole("combobox", { name: /pipeline/i });
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["All", "ci.yml", "nightly.yml"]);
    expect(select).toHaveValue("nightly.yml");
    expect(screen.getByRole("combobox", { name: /issue/i })).toHaveValue("none");

    // Choosing another pipeline rewrites the URL and refetches the leaderboard.
    await userEvent.selectOptions(select, "ci.yml");
    await act(async () => {});
    expect(window.location.search).toContain("pipeline=ci.yml");
    expect(urls.filter((u) => u.startsWith("/api/jobs?")).at(-1)).toContain("pipeline=ci.yml");
  });

  it("sends the stored search term to /api/jobs and says no jobs match", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 0,
            flaky_jobs: 0,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 0,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
          });
        if (url.startsWith("/api/jobs"))
          return json({ items: [], total: 0, page: 1, page_size: 50 });
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({});
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&q=e2e");
    render(<App />);

    expect(await screen.findByText("No jobs match.")).toBeInTheDocument();
    expect(urls.filter((u) => u.startsWith("/api/jobs?")).at(-1)).toContain("q=e2e");
    expect(screen.getByPlaceholderText("Job or pipeline")).toHaveValue("e2e");
  });

  it("sends the debounced search term as q and keeps the URL in sync", async () => {
    vi.useFakeTimers();
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    render(<App />);
    await act(async () => {});

    const search = (url: string) => new URL(url, "http://localhost").searchParams.get("q");
    const searched = () => urls.filter((u) => u.startsWith("/api/tests") && search(u) !== null);
    const input = screen.getByPlaceholderText("Name, classname or file");

    // The input is immediate; the request is not.
    fireEvent.change(input, { target: { value: "log" } });
    expect((input as HTMLInputElement).value).toBe("log");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    // A new keystroke restarts the 300 ms window, so nothing has been sent yet.
    fireEvent.change(input, { target: { value: "login" } });
    expect((input as HTMLInputElement).value).toBe("login");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    expect(searched()).toEqual([]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(searched().at(-1)).toContain("q=login");
    expect(window.location.search).toBe("?q=login");
  });

  it("sends the quarantined and issue filters and keeps the URL in sync", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    render(<App />);
    await act(async () => {});
    const testsUrl = () => urls.filter((u) => u.startsWith("/api/tests")).at(-1) ?? "";
    const param = (url: string, name: string) =>
      new URL(url, "http://localhost").searchParams.get(name);

    await userEvent.click(screen.getByRole("checkbox", { name: /quarantined only/i }));
    await act(async () => {});
    expect(param(testsUrl(), "quarantined")).toBe("true");
    expect(window.location.search).toBe("?quarantined=1");

    await userEvent.selectOptions(screen.getByRole("combobox", { name: /issue/i }), "none");
    await act(async () => {});
    // "none" means Tests with no Issue link, so has_issue=false.
    expect(param(testsUrl(), "has_issue")).toBe("false");
    expect(window.location.search).toBe("?quarantined=1&issue=none");
    // The Pipeline filter belongs to the Jobs view only.
    expect(screen.queryByRole("combobox", { name: /pipeline/i })).not.toBeInTheDocument();
  });

  it("sends broken for the broken-on-default-branch toggle, in the Tests view only", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 0,
            flaky_jobs: 0,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 0,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
            pipelines: [],
          });
        if (url.startsWith("/api/jobs"))
          return json({ items: [], total: 0, page: 1, page_size: 50 });
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(testsSummary);
      }),
    );
    render(<App />);
    await act(async () => {});
    const lastUrl = (prefix: string) => urls.filter((u) => u.startsWith(prefix)).at(-1) ?? "";
    const param = (url: string, name: string) =>
      new URL(url, "http://localhost").searchParams.get(name);
    const toggle = () => screen.getByRole("checkbox", { name: /broken on default branch/i });

    // Off by default: the API is asked without the parameter.
    expect(toggle()).not.toBeChecked();
    expect(param(lastUrl("/api/tests"), "broken")).toBeNull();

    await userEvent.click(toggle());
    await act(async () => {});
    expect(param(lastUrl("/api/tests"), "broken")).toBe("true");
    expect(window.location.search).toBe("?broken=1");

    // Toggling back clears the URL and the parameter.
    await userEvent.click(toggle());
    await act(async () => {});
    expect(param(lastUrl("/api/tests"), "broken")).toBeNull();
    expect(window.location.search).toBe("");

    // Broken on the default branch is a Test property, so the Jobs view drops the control.
    await userEvent.click(screen.getByRole("button", { name: "jobs" }));
    await act(async () => {});
    expect(
      screen.queryByRole("checkbox", { name: /broken on default branch/i }),
    ).not.toBeInTheDocument();
  });

  it("sends new_within_days for the new-flaky toggle, in both views", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/jobs/summary"))
          return json({
            total_jobs: 1,
            flaky_jobs: 1,
            suspect_jobs: 0,
            confirmed_flaky_jobs: 0,
            job_executions_30d: 4,
            job_failures_30d: 0,
            flake_threshold: 0.3,
            new_flaky_7d: 0,
            pipelines: [],
          });
        if (url.startsWith("/api/jobs"))
          return json({ items: [], total: 0, page: 1, page_size: 50 });
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    render(<App />);
    await act(async () => {});
    const lastUrl = (prefix: string) => urls.filter((u) => u.startsWith(prefix)).at(-1) ?? "";
    const param = (url: string, name: string) =>
      new URL(url, "http://localhost").searchParams.get(name);
    const toggle = () => screen.getByRole("checkbox", { name: /new in the last 7 days/i });

    // Off by default: the API is asked without the parameter.
    expect(param(lastUrl("/api/tests"), "new_within_days")).toBeNull();

    await userEvent.click(toggle());
    await act(async () => {});
    expect(param(lastUrl("/api/tests"), "new_within_days")).toBe("7");
    expect(window.location.search).toBe("?new=7");

    // Toggling back clears the URL and the parameter.
    await userEvent.click(toggle());
    await act(async () => {});
    expect(param(lastUrl("/api/tests"), "new_within_days")).toBeNull();
    expect(window.location.search).toBe("");

    // Jobs view: the same toggle, sent to /api/jobs.
    await userEvent.click(toggle());
    await act(async () => {});
    await userEvent.click(screen.getByRole("button", { name: "jobs" }));
    await act(async () => {});
    expect(param(lastUrl("/api/jobs"), "new_within_days")).toBe("7");
    expect(window.location.search).toBe("?view=jobs&new=7");
  });

  /**
   * The Tests list, with the row's quarantine state chosen by the caller. The
   * optional `acknowledgedUntil` sets the row's and the drawer history's
   * acknowledgement end date, so the drawer shows the matching button and the
   * lines under it. Like `stubJobsAck` below, the mutation POSTs move the state
   * every later read — the drawer's history and the list alike — answers from.
   */
  function stubTests(quarantined: boolean, acknowledgedUntil: string | null = null) {
    const calls: { url: string; body: Record<string, unknown> | null }[] = [];
    /** The fields the quarantine and acknowledge endpoints own on a Test row. */
    type Mutated = Pick<
      TestCase,
      | "quarantined"
      | "quarantined_at"
      | "quarantine_reason"
      | "quarantine_expires_at"
      | "acknowledged_until"
      | "acknowledge_reason"
    >;
    // A row already in a decided state carries the reason and expiry that
    // state implies, so the drawer has something to show — and, after a
    // one-click clear, something to stop showing.
    let mutated: Mutated = {
      quarantined,
      quarantined_at: quarantined ? "2026-09-25T00:00:00Z" : null,
      quarantine_reason: quarantined ? "flaky on CI" : null,
      quarantine_expires_at: quarantined ? "2999-01-01T00:00:00Z" : null,
      acknowledged_until: acknowledgedUntil,
      acknowledge_reason: acknowledgedUntil == null ? null : "known flake",
    };
    const rowNow = (): TestCase => ({ ...page("login").items[0], ...mutated });
    // The expiry the api would record for a quarantine of N days, measured from
    // now so the drawer's "expires in N days" line reads the same N.
    const expiresAt = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString();
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        const body =
          init?.body == null ? null : (JSON.parse(String(init.body)) as Record<string, unknown>);
        calls.push({ url, body });
        if (url.includes("/quarantine")) {
          mutated =
            body?.quarantined === true
              ? {
                  ...mutated,
                  quarantined: true,
                  quarantined_at: "2026-09-25T00:00:00Z",
                  quarantine_reason: typeof body.reason === "string" ? body.reason : null,
                  quarantine_expires_at:
                    typeof body.expires_in_days === "number"
                      ? expiresAt(body.expires_in_days)
                      : null,
                }
              : {
                  ...mutated,
                  quarantined: false,
                  quarantined_at: null,
                  quarantine_reason: null,
                  quarantine_expires_at: null,
                };
          return json(rowNow());
        }
        if (url.includes("/acknowledge")) {
          mutated =
            body?.acknowledged === true
              ? {
                  ...mutated,
                  acknowledged_until: "2999-01-01T00:00:00Z",
                  acknowledge_reason: typeof body.reason === "string" ? body.reason : null,
                }
              : { ...mutated, acknowledged_until: null, acknowledge_reason: null };
          return json(rowNow());
        }
        if (url.includes("/history"))
          return json(testHistory(5, { test: { ...testHistory(5).test, ...mutated } }));
        if (url.startsWith("/api/tests"))
          return json({ items: [rowNow()], total: 1, page: 1, page_size: 50 });
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json({
          total_tests: 1,
          flaky_tests: 1,
          suspect_tests: 0,
          confirmed_flaky_tests: 0,
          total_runs: 1,
          executions_30d: 1,
          failures_30d: 0,
          flake_threshold: 0.3,
          new_flaky_7d: 0,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        });
      }),
    );
    const posts = () => calls.filter((c) => c.url.includes("/quarantine"));
    const acks = () => calls.filter((c) => c.url.includes("/acknowledge"));
    const lists = () => calls.filter((c) => c.url.startsWith("/api/tests?")).length;
    const histories = () => calls.filter((c) => c.url.startsWith("/api/tests/5/history")).length;
    return { posts, acks, lists, histories };
  }

  // Mutation wiring: each trigger surface opens the right dialog, the direct
  // path posts the api's reset body, and the confirm path posts the body the
  // dialog collected — the choice-to-api mapping lives in App's declarations.
  // The flow rules themselves — nothing posts until Confirm, cancel records
  // nothing, the pending entity clears whatever the answer, the refresh and
  // drawer reloads after a settled confirm, the error mapping — are pinned by
  // the confirmed-decision module's own suite (confirmedDecision.test.ts).
  // The user-visible failure surface stays asserted here too: a rejecting
  // acknowledge POST closes the dialog and reaches the role="alert" banner,
  // like the quarantine cases in App.refresh.test.tsx.
  it("the leaderboard Quarantine button opens the Quarantine dialog", async () => {
    stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Quarantine" }));
    expect(await screen.findByRole("dialog", { name: /Quarantine login/ })).toBeInTheDocument();
  });

  it("the drawer Quarantine button opens the Quarantine dialog", async () => {
    stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();

    await userEvent.click(within(drawer).getByRole("button", { name: "Quarantine" }));
    expect(await screen.findByRole("dialog", { name: /Quarantine renders/ })).toBeInTheDocument();
  });

  it("un-quarantines with one click, posting the api's reset body", async () => {
    const { posts } = stubTests(true);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Un-quarantine" }));
    await act(async () => {});

    expect(posts()).toHaveLength(1);
    expect(posts()[0].body).toEqual({ quarantined: false });
  });

  it("a confirmed quarantine posts the reason and expiry the dialog collected", async () => {
    const { posts } = stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Quarantine" }));
    const dialog = await screen.findByRole("dialog", { name: /Quarantine login/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "flaky on CI");
    await userEvent.click(within(dialog).getByRole("radio", { name: "14 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    expect(posts()).toHaveLength(1);
    expect(posts()[0].url).toBe("/api/tests/5/quarantine");
    expect(posts()[0].body).toEqual({
      quarantined: true,
      reason: "flaky on CI",
      expires_in_days: 14,
    });
  });

  it("reloads the Test drawer after quarantining, so it shows the new state", async () => {
    const { posts, histories, lists } = stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();
    const historiesBefore = histories();
    const listsBefore = lists();

    await userEvent.click(within(drawer).getByRole("button", { name: "Quarantine" }));
    const dialog = await screen.findByRole("dialog", { name: /Quarantine renders/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "flaky on CI");
    await userEvent.click(within(dialog).getByRole("radio", { name: "14 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    expect(posts()).toHaveLength(1);
    expect(posts()[0].body).toEqual({
      quarantined: true,
      reason: "flaky on CI",
      expires_in_days: 14,
    });
    // Both the list and the open drawer reload, so neither keeps stale state.
    expect(histories()).toBeGreaterThan(historiesBefore);
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(
      await within(drawer).findByRole("button", { name: "Un-quarantine" }),
    ).toBeInTheDocument();
    expect(within(drawer).getByText(/Reason: flaky on CI/)).toBeInTheDocument();
    expect(within(drawer).getByText(/expires in 14 days/)).toBeInTheDocument();
  });

  it("reloads the Test drawer after un-quarantining, clearing the lines it showed", async () => {
    const { posts, histories, lists } = stubTests(true);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();
    const historiesBefore = histories();
    const listsBefore = lists();

    // The drawer starts by showing what the quarantine recorded.
    expect(within(drawer).getByText(/Reason: flaky on CI/)).toBeInTheDocument();
    expect(within(drawer).getByText(/expires in/)).toBeInTheDocument();

    await userEvent.click(within(drawer).getByRole("button", { name: "Un-quarantine" }));
    await act(async () => {});

    expect(posts()).toHaveLength(1);
    expect(posts()[0].body).toEqual({ quarantined: false });
    // Both the list and the open drawer reload, so neither keeps stale state.
    expect(histories()).toBeGreaterThan(historiesBefore);
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(await within(drawer).findByRole("button", { name: "Quarantine" })).toBeInTheDocument();
    expect(within(drawer).queryByText(/Reason: flaky on CI/)).not.toBeInTheDocument();
    expect(within(drawer).queryByText(/expires in/)).not.toBeInTheDocument();
  });

  /** Opens the Test drawer for the "login" row, like the leaderboard would. */
  async function openDrawer() {
    await userEvent.click(screen.getByText("login"));
    const drawer = await screen.findByRole("dialog", { name: "Test detail" });
    // The drawer's button acts on the history's Test, which the history fetch populates.
    await within(drawer).findByText("renders");
    return drawer;
  }

  it("the drawer Acknowledge button opens the Acknowledge dialog", async () => {
    stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    expect(await screen.findByRole("dialog", { name: /Acknowledge renders/ })).toBeInTheDocument();
  });

  it("clears an acknowledgement with one click, posting the api's reset body", async () => {
    const { acks } = stubTests(false, "2999-01-01T00:00:00Z");
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();

    await userEvent.click(within(drawer).getByRole("button", { name: "Clear acknowledgement" }));
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].body).toEqual({ acknowledged: false });
  });

  it("reloads the Test drawer after clearing an acknowledgement", async () => {
    const { acks, histories, lists } = stubTests(false, "2999-01-01T00:00:00Z");
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();
    const historiesBefore = histories();
    const listsBefore = lists();

    // The drawer starts by showing what the acknowledgement recorded.
    expect(within(drawer).getByText(/Reason: known flake/)).toBeInTheDocument();
    expect(within(drawer).getByText(/acknowledged until/)).toBeInTheDocument();

    await userEvent.click(within(drawer).getByRole("button", { name: "Clear acknowledgement" }));
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].body).toEqual({ acknowledged: false });
    // Both the list and the open drawer reload, so neither keeps stale state.
    expect(histories()).toBeGreaterThan(historiesBefore);
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(await within(drawer).findByRole("button", { name: "Acknowledge" })).toBeInTheDocument();
    expect(within(drawer).queryByText(/Reason: known flake/)).not.toBeInTheDocument();
    expect(within(drawer).queryByText(/acknowledged until/)).not.toBeInTheDocument();
  });

  it("a confirmed Test acknowledgement posts the reason and duration the dialog collected", async () => {
    const { acks } = stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge renders/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "known flake");
    await userEvent.click(within(dialog).getByRole("radio", { name: "30 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].url).toBe("/api/tests/5/acknowledge");
    expect(acks()[0].body).toEqual({ acknowledged: true, reason: "known flake", days: 30 });
  });

  it("reloads the Test drawer after acknowledging, so it shows the new state", async () => {
    const { acks, histories, lists } = stubTests(false);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();
    const historiesBefore = histories();
    const listsBefore = lists();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge renders/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "known flake");
    await userEvent.click(within(dialog).getByRole("radio", { name: "30 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].url).toBe("/api/tests/5/acknowledge");
    expect(acks()[0].body).toEqual({ acknowledged: true, reason: "known flake", days: 30 });
    // Both the list and the open drawer reload, so neither keeps stale state.
    expect(histories()).toBeGreaterThan(historiesBefore);
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(
      await within(drawer).findByRole("button", { name: "Clear acknowledgement" }),
    ).toBeInTheDocument();
    expect(within(drawer).getByText(/Reason: known flake/)).toBeInTheDocument();
    expect(within(drawer).getByText(/acknowledged until/)).toBeInTheDocument();
  });

  it("a drawer closed while its mutation is in flight updates only the list", async () => {
    const { acks, histories, lists, finishPost } = stubSlowTestAcknowledgePost();
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();
    const historiesBefore = histories();
    const listsBefore = lists();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge renders/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "known flake");
    await userEvent.click(within(dialog).getByRole("radio", { name: "30 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));

    // The POST is still in flight when the user closes the drawer, so by the
    // time the mutation settles nothing displays this row any more.
    expect(acks()).toBe(1);
    fireEvent.keyDown(window, { key: "Escape" });
    await act(async () => {});
    expect(screen.queryByRole("dialog", { name: "Test detail" })).not.toBeInTheDocument();

    finishPost({ ...page("login").items[0], acknowledged_until: "2999-01-01T00:00:00Z" });
    await act(async () => {});

    // Only the list refetches. The drawer's history count stays at its initial
    // load, because a disabled hook registers nothing and its own refresh is a
    // no-op; reloading a row nobody displays resolves quietly, with no banner.
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(histories()).toBe(historiesBefore);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /** The Tests list with its drawer history, and an acknowledge POST that fails. */
  function stubFailingTestAcknowledgePost() {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        // /acknowledge must be matched before the /api/tests list.
        if (url.includes("/acknowledge")) {
          return Promise.resolve(new Response("nope", { status: 500 }));
        }
        if (url.startsWith("/api/tests/5/history")) return json(testHistory(5));
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(testsSummary);
      }),
    );
  }

  /**
   * The same Tests view, with an acknowledge POST that stays in flight until the
   * test hands over its response — so a drawer can close while the mutation is
   * still running, which is what the closed-drawer test needs to observe.
   */
  function stubSlowTestAcknowledgePost() {
    const urls: string[] = [];
    let settlePost: (response: Response) => void = () => {};
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        // /acknowledge must be matched before the /api/tests list.
        if (url.includes("/acknowledge")) {
          return new Promise<Response>((resolve) => {
            settlePost = resolve;
          });
        }
        if (url.startsWith("/api/tests/5/history")) return json(testHistory(5));
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(testsSummary);
      }),
    );
    return {
      acks: () => urls.filter((u) => u.includes("/acknowledge")).length,
      histories: () => urls.filter((u) => u.startsWith("/api/tests/5/history")).length,
      lists: () => urls.filter((u) => u.startsWith("/api/tests?")).length,
      /** Settles the in-flight POST with the row the api would have returned. */
      finishPost: (body: unknown) => settlePost(new Response(JSON.stringify(body))),
    };
  }

  it("closes the Test acknowledge dialog and shows the banner when the POST fails", async () => {
    stubFailingTestAcknowledgePost();
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();
    const drawer = await openDrawer();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge renders/ });
    // The reason is optional, so confirming right away is a valid choice.
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    // The dialog closes before the POST, and the failure still reaches the banner.
    expect(screen.queryByRole("dialog", { name: /Acknowledge renders/ })).not.toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(/acknowledge tests 5 -> 500/);
  });

  /**
   * The Jobs view with Job 3's drawer open, whose acknowledgement starts at
   * `initial` and changes when the acknowledge POST arrives, so the drawer's
   * next history fetch shows what the mutation did.
   */
  function stubJobsAck(
    initial: {
      acknowledged_until: string | null;
      acknowledge_reason: string | null;
    },
    fail = false,
  ) {
    const calls: { url: string; body: Record<string, unknown> | null }[] = [];
    let ack = initial;
    const job = (): Job => ({ ...jobHistory().job, ...ack });
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        const body =
          init?.body == null ? null : (JSON.parse(String(init.body)) as Record<string, unknown>);
        calls.push({ url, body });
        if (url.includes("/acknowledge")) {
          if (fail) return Promise.resolve(new Response("nope", { status: 500 }));
          ack =
            body?.acknowledged === true
              ? {
                  acknowledged_until: "2999-01-01T00:00:00Z",
                  acknowledge_reason: typeof body.reason === "string" ? body.reason : null,
                }
              : { acknowledged_until: null, acknowledge_reason: null };
          return json(job());
        }
        if (url.startsWith("/api/jobs/3/history")) return json(jobHistory({ job: job() }));
        if (url.startsWith("/api/jobs/summary")) return json(jobSummaryFixture);
        if (url.startsWith("/api/jobs"))
          return json({
            ...jobPageFixture,
            items: [{ ...jobPageFixture.items[0], ...ack }],
          });
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(jobSummaryFixture);
      }),
    );
    return {
      acks: () => calls.filter((c) => c.url.includes("/acknowledge")),
      histories: () => calls.filter((c) => c.url.startsWith("/api/jobs/3/history")).length,
      lists: () => calls.filter((c) => c.url.startsWith("/api/jobs?")).length,
    };
  }

  it("reloads the Job drawer after acknowledging, so it shows the new state", async () => {
    const { acks, histories, lists } = stubJobsAck({
      acknowledged_until: null,
      acknowledge_reason: null,
    });
    window.history.replaceState(null, "", "/?view=jobs&job=3");
    render(<App />);
    const drawer = await screen.findByRole("dialog", { name: "Job detail" });
    await within(drawer).findByRole("button", { name: "Acknowledge" });
    const historiesBefore = histories();
    const listsBefore = lists();

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge e2e/ });
    await userEvent.type(within(dialog).getByRole("textbox", { name: /Reason/ }), "known flake");
    await userEvent.click(within(dialog).getByRole("radio", { name: "30 days" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].url).toBe("/api/jobs/3/acknowledge");
    expect(acks()[0].body).toEqual({ acknowledged: true, reason: "known flake", days: 30 });
    // Both the list and the open drawer reload, so neither keeps stale state.
    expect(histories()).toBeGreaterThan(historiesBefore);
    expect(lists()).toBeGreaterThan(listsBefore);
    expect(
      await within(drawer).findByRole("button", { name: "Clear acknowledgement" }),
    ).toBeInTheDocument();
    expect(within(drawer).getByText(/known flake/)).toBeInTheDocument();
  });

  it("the Job drawer's Acknowledge button opens the Acknowledge dialog", async () => {
    stubJobsAck({ acknowledged_until: null, acknowledge_reason: null });
    const drawer = await openJobDrawer();

    await userEvent.click(await within(drawer).findByRole("button", { name: "Acknowledge" }));
    expect(await screen.findByRole("dialog", { name: /Acknowledge e2e/ })).toBeInTheDocument();
  });

  it("clears a Job's acknowledgement with one click, posting the api's reset body", async () => {
    const { acks } = stubJobsAck({
      acknowledged_until: "2999-01-01T00:00:00Z",
      acknowledge_reason: "known flake",
    });
    const drawer = await openJobDrawer();

    await userEvent.click(
      await within(drawer).findByRole("button", { name: "Clear acknowledgement" }),
    );
    await act(async () => {});

    expect(acks()).toHaveLength(1);
    expect(acks()[0].body).toEqual({ acknowledged: false });
  });

  it("closes the Job acknowledge dialog and shows the banner when the POST fails", async () => {
    stubJobsAck(
      {
        acknowledged_until: null,
        acknowledge_reason: null,
      },
      true,
    );
    const drawer = await openJobDrawer();
    await within(drawer).findByRole("button", { name: "Acknowledge" });

    await userEvent.click(within(drawer).getByRole("button", { name: "Acknowledge" }));
    const dialog = await screen.findByRole("dialog", { name: /Acknowledge e2e/ });
    await userEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await act(async () => {});

    // The dialog closes before the POST, and the failure still reaches the banner.
    expect(screen.queryByRole("dialog", { name: /Acknowledge e2e/ })).not.toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(/acknowledge jobs 3 -> 500/);
  });

  /** Opens Job 3's drawer on the Jobs view, like the leaderboard would. */
  async function openJobDrawer() {
    window.history.replaceState(null, "", "/?view=jobs&job=3");
    render(<App />);
    return await screen.findByRole("dialog", { name: "Job detail" });
  }

  // ---- Navigation wiring (issue #80): drawers, views, sort, filters, paging ----

  it("opens a Test drawer from a leaderboard row and loads its history", async () => {
    const urls = stubOrdered([
      ["/api/tests/5/history", () => json(testHistory(5))],
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await userEvent.click(await screen.findByText("login"));
    await act(async () => {});

    expect(window.location.search).toBe("?test=5");
    expect(urls).toContain("/api/tests/5/history?limit=60");
    const dialog = await screen.findByRole("dialog", { name: "Test detail" });
    // The drawer's history fetch populates the name, so poll for it.
    expect(await within(dialog).findByText("renders")).toBeInTheDocument();
  });

  it("closes the Test drawer on Escape, clearing its id from the URL", async () => {
    stubOrdered([
      ["/api/tests/5/history", () => json(testHistory(5))],
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await userEvent.click(await screen.findByText("login"));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "Test detail" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await act(async () => {});
    expect(window.location.search).toBe("");
    expect(screen.queryByRole("dialog", { name: "Test detail" })).not.toBeInTheDocument();
  });

  it("opens a Job drawer from a Jobs leaderboard row and loads its history", async () => {
    const urls = stubOrdered([
      ["/api/jobs/3/history", () => json(jobHistory())],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
    ]);
    window.history.replaceState(null, "", "/?view=jobs");
    render(<App />);
    await userEvent.click(await screen.findByText("e2e"));
    await act(async () => {});

    expect(window.location.search).toBe("?view=jobs&job=3");
    expect(urls).toContain("/api/jobs/3/history?limit=60");
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
  });

  it("closes the Job drawer on Escape, clearing its id from the URL", async () => {
    stubOrdered([
      ["/api/jobs/3/history", () => json(jobHistory())],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
    ]);
    window.history.replaceState(null, "", "/?view=jobs&job=3");
    render(<App />);
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await act(async () => {});
    expect(window.location.search).toBe("?view=jobs");
    expect(screen.queryByRole("dialog", { name: "Job detail" })).not.toBeInTheDocument();
  });

  it("opens a Job drawer from a Test drawer's Seen in Jobs button", async () => {
    const urls = stubOrdered([
      [
        "/api/tests/5/history",
        () =>
          json(
            testHistory(5, {
              jobs: [{ job_id: 3, pipeline: ".github/workflows/ci.yml", name: "e2e" }],
            }),
          ),
      ],
      ["/api/jobs/3/history", () => json(jobHistory())],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await userEvent.click(await screen.findByText("login"));
    await act(async () => {});
    await userEvent.click(
      await screen.findByRole("button", { name: ".github/workflows/ci.yml · e2e" }),
    );
    await act(async () => {});

    expect(window.location.search).toBe("?view=jobs&job=3");
    expect(urls).toContain("/api/jobs/3/history?limit=60");
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
  });

  it("opens a Test drawer from a Job drawer's explaining-test button", async () => {
    const urls = stubOrdered([
      ["/api/jobs/3/history", () => json(jobHistory())],
      ["/api/tests/9/history", () => json(testHistory(9))],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    window.history.replaceState(null, "", "/?view=jobs&job=3");
    render(<App />);
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
    await userEvent.click(
      await screen.findByRole("button", { name: "backend · test_login (failed)" }),
    );
    await act(async () => {});

    expect(window.location.search).toBe("?test=9");
    expect(urls).toContain("/api/tests/9/history?limit=60");
    expect(await screen.findByRole("dialog", { name: "Test detail" })).toBeInTheDocument();
  });

  it("opens a sibling Job from a summary Job drawer's badge", async () => {
    const urls = stubOrdered([
      [
        "/api/jobs/3/history",
        () =>
          json(
            jobHistory({
              job: { ...jobHistory().job, summary: true, name: "alls-green" },
              executions: [
                {
                  ...jobHistory().executions[0],
                  explained_by: [],
                  explained_by_jobs: [
                    {
                      job_id: 4,
                      pipeline: ".github/workflows/ci.yml",
                      name: "test (ubuntu)",
                      url: "https://github.com/acme/app/actions/runs/1/jobs/2",
                    },
                  ],
                },
              ],
            }),
          ),
      ],
      ["/api/jobs/4/history", () => json(jobHistory())],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
    ]);
    window.history.replaceState(null, "", "/?view=jobs&job=3");
    render(<App />);
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
    await userEvent.click(
      await screen.findByRole("button", { name: ".github/workflows/ci.yml · test (ubuntu)" }),
    );
    await act(async () => {});

    expect(window.location.search).toBe("?view=jobs&job=4");
    expect(urls).toContain("/api/jobs/4/history?limit=60");
  });

  it("writes the selected repo to the URL and the next request", async () => {
    const urls = stubOrdered([
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([{ name: "a/b", projects: [{ name: "backend", root: "" }] }])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await act(async () => {});

    await userEvent.selectOptions(screen.getByLabelText("Repo"), "a/b");
    await act(async () => {});
    expect(window.location.search).toBe("?repo=a%2Fb");
    const last = urls.filter((u) => u.startsWith("/api/tests")).at(-1) ?? "";
    expect(new URL(last, "http://localhost").searchParams.get("repo")).toBe("a/b");
  });

  it("wires sort, show-stable and cause to the URL and the request", async () => {
    const urls = stubOrdered([
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await act(async () => {});
    const last = (prefix: string) => urls.filter((u) => u.startsWith(prefix)).at(-1) ?? "";
    const param = (url: string, name: string) =>
      new URL(url, "http://localhost").searchParams.get(name);

    await userEvent.selectOptions(screen.getByLabelText("Sort by"), "proven");
    await act(async () => {});
    expect(param(last("/api/tests"), "sort")).toBe("proven");
    expect(window.location.search).toBe("?sort=proven");

    await userEvent.click(screen.getByRole("checkbox", { name: "Show stable tests" }));
    await act(async () => {});
    expect(param(last("/api/tests"), "include_stable")).toBe("true");
    expect(window.location.search).toBe("?sort=proven&stable=1");

    await userEvent.selectOptions(screen.getByLabelText("Likely cause"), "timing");
    await act(async () => {});
    expect(param(last("/api/tests"), "category")).toBe("timing");
    expect(window.location.search).toBe("?sort=proven&stable=1&cause=timing");
  });

  it("wires the acknowledged filter to the URL and the request", async () => {
    const urls = stubOrdered([
      ["/api/tests", () => json(page("login"))],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await act(async () => {});
    const param = (prefix: string, name: string) =>
      new URL(
        urls.filter((u) => u.startsWith(prefix)).at(-1) ?? "",
        "http://localhost",
      ).searchParams.get(name);

    await userEvent.click(screen.getByRole("checkbox", { name: "Acknowledged only" }));
    await act(async () => {});
    expect(param("/api/tests", "acknowledged")).toBe("true");
    expect(window.location.search).toBe("?acknowledged=1");

    // The Jobs view keeps the filter and sends it to /api/jobs too.
    await userEvent.click(screen.getByRole("button", { name: "jobs" }));
    await act(async () => {});
    expect(param("/api/jobs", "acknowledged")).toBe("true");
    expect(window.location.search).toBe("?view=jobs&acknowledged=1");

    // Unchecking clears the URL and the parameter.
    await userEvent.click(screen.getByRole("checkbox", { name: "Acknowledged only" }));
    await act(async () => {});
    expect(window.location.search).toBe("?view=jobs");
  });

  it("paginates, writing the page to the URL and the request", async () => {
    const urls = stubOrdered([
      [
        "/api/tests",
        () => json({ items: [page("login").items[0]], total: 120, page: 1, page_size: 50 }),
      ],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    await act(async () => {});

    await userEvent.click(screen.getByRole("button", { name: /Next/ }));
    await act(async () => {});
    expect(window.location.search).toBe("?page=2");
    const last = urls.filter((u) => u.startsWith("/api/tests")).at(-1) ?? "";
    expect(new URL(last, "http://localhost").searchParams.get("page")).toBe("2");
  });

  it("writes the picked CI run attempt to the URL", async () => {
    stubOrdered([
      ["/api/ci-runs/", () => json(ciRunBody())],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
    ]);
    window.history.replaceState(null, "", "/?view=ci-run&repo=a%2Fb&ci_run=99");
    render(<App />);
    expect(await screen.findByRole("button", { name: "test_login" })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Attempt"), { target: { value: "3" } });
    await act(async () => {});
    expect(window.location.search).toBe("?repo=a%2Fb&view=ci-run&ci_run=99&attempt=3");
  });

  it("opens the Test drawer from a CI run failure row", async () => {
    const urls = stubOrdered([
      ["/api/ci-runs/", () => json(ciRunBody())],
      ["/api/tests/1/history", () => json(testHistory(1))],
      ["/api/tests", () => json(page("login"))],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    window.history.replaceState(null, "", "/?view=ci-run&repo=a%2Fb&ci_run=99");
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "test_login" }));
    await act(async () => {});

    expect(window.location.search).toBe("?repo=a%2Fb&test=1");
    expect(urls).toContain("/api/tests/1/history?limit=60");
    expect(await screen.findByRole("dialog", { name: "Test detail" })).toBeInTheDocument();
  });

  it("opens the Job drawer from a CI run failed-job row", async () => {
    const urls = stubOrdered([
      ["/api/ci-runs/", () => json(ciRunBody())],
      ["/api/jobs/5/history", () => json(jobHistory())],
      ["/api/jobs/summary", () => json(jobSummaryFixture)],
      ["/api/jobs", () => json(jobPageFixture)],
      ["/api/repos", () => json([])],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
    ]);
    window.history.replaceState(null, "", "/?view=ci-run&repo=a%2Fb&ci_run=99");
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: /test \(ubuntu\)/ }));
    await act(async () => {});

    expect(window.location.search).toBe("?repo=a%2Fb&view=jobs&job=5");
    expect(urls).toContain("/api/jobs/5/history?limit=60");
    expect(await screen.findByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
  });

  it("submitting an empty CI run form changes nothing in the URL", async () => {
    stubOrdered([
      ["/api/repos", () => json([{ name: "a/b", projects: [{ name: "backend", root: "" }] }])],
      ["/api/tests", () => json(page("login"))],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    window.history.replaceState(null, "", "/?repo=a%2Fb");
    render(<App />);

    const input = await screen.findByLabelText("Open CI run");
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.submit(input.closest("form")!);
    await act(async () => {});
    expect(window.location.search).toBe("?repo=a%2Fb");
  });

  it("links the footer to the README CI integration section instead of a curl command", async () => {
    stubOrdered([
      ["/api/repos", () => json([{ name: "a/b", projects: [{ name: "backend", root: "" }] }])],
      ["/api/tests", () => json(page("login"))],
      ["/api/reports/summary", () => json({ pending: 0, failed: 0 })],
      ["/api/summary", () => json(testsSummary)],
    ]);
    render(<App />);
    expect(await screen.findByText("login")).toBeInTheDocument();

    // The footer must not show a partial ingest command: a copy that omits
    // ci_run_id, ci_job_id, pipeline and default_branch breaks job attribution.
    const footer = screen.getByRole("contentinfo");
    expect(footer.textContent).not.toMatch(/curl|api\/ingest/);
    const link = within(footer).getByRole("link", { name: /README/ });
    expect(link).toHaveAttribute(
      "href",
      "https://github.com/andrewthetechie/flakeradar/blob/main/README.md#ci-integration",
    );
  });
});
