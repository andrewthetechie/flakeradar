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
});
