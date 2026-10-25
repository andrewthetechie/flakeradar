import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TestCase, TestPage } from "./api";
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
    trend: null,
    last_status: "failed",
    last_seen_at: "2026-09-25T00:00:00Z",
    quarantined: false,
    quarantined_at: null,
    github_issue_number: null,
    github_issue_url: null,
  };
  return { items: [test], total: 1, page: 1, page_size: 50 };
}

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body)));

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
          total_executions: 1,
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
          trend: null,
          last_status: "failed",
          last_seen_at: "2026-09-25T00:00:00Z",
          github_issue_number: null,
          github_issue_url: null,
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
      total_job_executions: 10,
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
          status: "passed",
          failed_attempts: 1,
          failure_category: null,
          message: "Connection refused",
          tier: "flaky",
          flakiness_score: 0.5,
          confirmed_flake_count: 1,
          clean_streak: 0,
          quarantined: false,
          github_issue_number: null,
          github_issue_url: null,
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
          total_executions: 1,
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
        trend: null,
        last_status: "failed",
        last_seen_at: "2026-09-25T00:00:00Z",
        summary: false,
        github_issue_number: null,
        github_issue_url: null,
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
            total_job_executions: 1,
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
            total_job_executions: 4,
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
            total_job_executions: 0,
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
          total_executions: 1,
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
          total_executions: 1,
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
            total_job_executions: 4,
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
          total_executions: 1,
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
});
