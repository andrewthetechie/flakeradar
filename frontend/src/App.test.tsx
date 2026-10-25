import { act, fireEvent, render, screen } from "@testing-library/react";
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
});
