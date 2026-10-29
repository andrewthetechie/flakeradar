// Refresh lifecycle of the App dashboard (issue #81): the 30 s poll, the API
// error banner and the "only the newest refresh may write state" guards.
// The helpers below are copies of the ones in src/App.test.tsx — tests never
// import from each other.
import { act, render, screen } from "@testing-library/react";
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
    quarantine_reason: null,
    quarantine_expires_at: null,
    github_issue_number: null,
    github_issue_url: null,
  };
  return { items: [test], total: 1, page: 1, page_size: 50 };
}

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body)));

// The default /api/summary body, as the other App tests serve it.
const summary = {
  total_tests: 1,
  flaky_tests: 1,
  suspect_tests: 0,
  confirmed_flaky_tests: 0,
  total_runs: 1,
  total_executions: 1,
  flake_threshold: 0.3,
  new_flaky_7d: 0,
  category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
};

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

describe("App refresh", () => {
  it("polls every 30 s and stops when unmounted", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        urls.push(url);
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([{ name: "x/y", projects: [] }]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(summary);
      }),
    );
    // Frozen clock: the interval is the only timer this view owns, so every
    // step is an explicit advance. findBy* is unusable under fake timers.
    vi.useFakeTimers();
    const { unmount } = render(<App />);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(urls.filter((u) => u.startsWith("/api/tests?"))).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(urls.filter((u) => u.startsWith("/api/tests?"))).toHaveLength(2);

    // The cleanup clears the interval: no further request after unmount.
    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(urls.filter((u) => u.startsWith("/api/tests?"))).toHaveLength(2);
  });

  it("shows the API error banner for a 500 and clears it on the next good refresh", async () => {
    let fail = true;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/tests")) {
          return fail
            ? Promise.resolve(new Response("boom", { status: 500 }))
            : json(page("login"));
        }
        if (url.startsWith("/api/repos")) return json([{ name: "x/y", projects: [] }]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(summary);
      }),
    );
    // Fake timers from the first render: the recovery is the 30 s poll, and a
    // real interval installed before vi.useFakeTimers() would not be advanced.
    vi.useFakeTimers();
    render(<App />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    const banner = screen.getByRole("alert");
    expect(banner.textContent).toBe(
      "Could not reach the FlakeRadar API (/api/tests?page=1&page_size=50&sort=score -> 500). " +
        "Is the backend running on port 8000?",
    );

    // A healthy poll clears the banner: the success arm resets the error.
    fail = false;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("renders a rejection that is not an Error instance", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/summary")) return Promise.reject("offline");
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(summary);
      }),
    );
    render(<App />);

    const banner = await screen.findByRole("alert");
    expect(banner.textContent).toContain("(offline)");
  });

  it("ignores a stale repos response after leaving the CI run view", async () => {
    let finishOld: (r: Response) => void = () => {};
    let reposCalls = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/ci-runs/")) {
          // CiRunView stays loading, so only the header's repos can race.
          return new Promise<Response>(() => {});
        }
        if (url.startsWith("/api/tests")) return json(page("login"));
        if (url.startsWith("/api/repos")) {
          reposCalls++;
          if (reposCalls === 1) {
            return new Promise<Response>((resolve) => {
              finishOld = resolve;
            });
          }
          return json([{ name: "new/repo", projects: [] }]);
        }
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(summary);
      }),
    );
    window.history.replaceState(null, "", "/?view=ci-run&repo=a%2Fb&ci_run=99");
    render(<App />);

    // Back to the leaderboard while the CI run view's first request is pending.
    act(() => {
      window.history.pushState(null, "", "/");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(await screen.findByRole("option", { name: "new/repo" })).toBeInTheDocument();

    await act(async () => {
      finishOld(new Response(JSON.stringify([{ name: "stale/repo", projects: [] }])));
    });
    expect(screen.queryByRole("option", { name: "stale/repo" })).toBeNull();
    expect(screen.getByRole("option", { name: "new/repo" })).toBeInTheDocument();
  });

  it("ignores a stale Jobs response for a repo the user already left", async () => {
    // The Job fixture of src/App.test.tsx, with a name we can look for.
    const jobs = (name: string) => ({
      items: [
        {
          id: 3,
          repo: "x/y",
          provider: "github",
          pipeline: ".github/workflows/ci.yml",
          name,
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
    });
    const jobSummary = {
      total_jobs: 1,
      flaky_jobs: 1,
      suspect_jobs: 0,
      confirmed_flaky_jobs: 0,
      total_job_executions: 10,
      flake_threshold: 0.3,
      new_flaky_7d: 0,
    };
    let finishOld: (r: Response) => void = () => {};
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        // /api/jobs/summary must be matched before the /api/jobs list.
        if (url.startsWith("/api/jobs/summary")) return json(jobSummary);
        if (url.startsWith("/api/jobs") && url.includes("repo=a%2Fone")) {
          return new Promise<Response>((resolve) => {
            finishOld = resolve;
          });
        }
        if (url.startsWith("/api/jobs")) return json(jobs("newjob"));
        if (url.startsWith("/api/repos")) return json([]);
        if (url.startsWith("/api/reports/summary")) return json({ pending: 0, failed: 0 });
        return json(summary);
      }),
    );
    window.history.replaceState(null, "", "/?view=jobs&repo=a%2Fone");
    render(<App />);

    act(() => {
      window.history.pushState(null, "", "/?view=jobs&repo=b%2Ftwo");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(await screen.findByText("newjob")).toBeInTheDocument();

    await act(async () => {
      finishOld(new Response(JSON.stringify(jobs("oldjob"))));
    });
    expect(screen.queryByText("oldjob")).not.toBeInTheDocument();
    expect(screen.getByText("newjob")).toBeInTheDocument();
  });

  it("raises no banner for a failure the user has already superseded", async () => {
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
        return json(summary);
      }),
    );
    window.history.replaceState(null, "", "/?repo=a%2Fone");
    render(<App />);

    act(() => {
      window.history.pushState(null, "", "/?repo=b%2Ftwo");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(await screen.findByText("newtest")).toBeInTheDocument();

    // The abandoned request fails after the new one rendered: its error is
    // dropped with the response it belongs to.
    await act(async () => {
      finishOld(new Response("boom", { status: 500 }));
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
