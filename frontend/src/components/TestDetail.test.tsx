import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { History } from "../api";
import { TestDetail } from "./TestDetail";

// Executions are newest first, as the API returns them.
const history: History = {
  test: {
    id: 7,
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
    trend: "steady",
    last_status: "passed",
    last_seen_at: "2026-09-25T00:00:00Z",
    quarantined: false,
    quarantined_at: null,
    quarantine_reason: null,
    quarantine_expires_at: null,
    github_issue_number: null,
    github_issue_url: null,
  },
  location: null,
  last_failing_sha: "aaa1110000",
  last_failing_branch: "main",
  jobs: [],
  executions: [
    {
      id: 3,
      status: "error",
      duration: 0,
      message: "",
      details: "Traceback\n  at src/app.test.ts:14",
      created_at: "2026-09-25T02:00:00Z",
      commit_sha: "bbb2220000aaaa",
      branch: "feat",
      ci_run_id: "2",
      attempt: 0,
      failure_category: "assertion",
    },
    {
      id: 2,
      status: "passed",
      duration: 1.5,
      message: "hello msg",
      details: "",
      created_at: "2026-09-25T01:00:00Z",
      commit_sha: "aaa1110000bbbb",
      branch: "main",
      ci_run_id: "2",
      attempt: 0,
      failure_category: null,
    },
  ],
  score_history: [
    {
      day: "2026-09-20",
      flakiness_score: 0.6,
      confirmed_flake_count: 1,
      executions: 2,
      failures: 1,
    },
  ],
};

function renderDetail(overrides: Partial<History> = {}) {
  const onOpenJob = vi.fn();
  const onOpenCiRun = vi.fn();
  const view = render(
    <TestDetail
      history={{ ...history, ...overrides }}
      onOpenJob={onOpenJob}
      onOpenCiRun={onOpenCiRun}
    />,
  );
  return { ...view, onOpenJob, onOpenCiRun };
}

/** The strip's invisible hit targets, oldest (visually leftmost) first. */
function hoverTargets(container: HTMLElement) {
  return Array.from(container.querySelectorAll('rect[fill="transparent"]'));
}

describe("TestDetail", () => {
  it("shows a tooltip with duration, commit, branch and message on hover", () => {
    const { container } = renderDetail();
    const svg = screen.getByRole("img", { name: /Execution history/ });
    const targets = hoverTargets(container);
    // The strip draws oldest -> newest from a newest-first array, so the first
    // target is the last execution in the fixture.
    expect(targets).toHaveLength(2);

    fireEvent.mouseOver(targets[0]);

    const tooltip = svg.nextElementSibling;
    expect(tooltip).not.toBeNull();
    expect(tooltip!.textContent).toContain("passed · 1.50s");
    expect(tooltip!.textContent).toContain("aaa1110000 on main");
    expect(tooltip!.textContent).toContain("hello msg");
  });

  it("omits the duration and message lines for a zero-duration failure", () => {
    const { container } = renderDetail();
    const svg = screen.getByRole("img", { name: /Execution history/ });

    fireEvent.mouseOver(hoverTargets(container)[1]);

    const tooltip = svg.nextElementSibling;
    expect(tooltip).not.toBeNull();
    expect(tooltip!.textContent).toContain("error");
    expect(tooltip!.textContent).toContain("bbb2220000 on feat");
    expect(tooltip!.textContent).not.toMatch(/\d\.\d\ds/);
    expect(tooltip!.textContent).not.toContain("hello msg");
  });

  it("removes the tooltip when the pointer leaves the strip", () => {
    const { container } = renderDetail();
    const svg = screen.getByRole("img", { name: /Execution history/ });

    fireEvent.mouseOver(hoverTargets(container)[0]);
    expect(svg.nextElementSibling).not.toBeNull();

    fireEvent.mouseOut(svg);
    expect(svg.nextElementSibling).toBeNull();
  });

  it("treats an error execution as the latest failure", () => {
    renderDetail();
    expect(screen.getByText(/Latest failure/)).toBeInTheDocument();
    expect(screen.getByText(/Traceback/)).toBeInTheDocument();
  });

  it("shows a zero proven-flake count without highlighting it", () => {
    renderDetail({ test: { ...history.test, confirmed_flake_count: 0 } });
    const tile = within(screen.getByText("Proven flakes").parentElement!);
    expect(tile.getByText("0")).toBeInTheDocument();
  });

  it("shows an em dash when the test has no failure category", () => {
    renderDetail({ test: { ...history.test, failure_category: null } });
    const tile = within(screen.getByText("Likely cause").parentElement!);
    expect(tile.getByText("—")).toBeInTheDocument();
  });

  it("separates two Seen in Jobs entries and opens the clicked Job", async () => {
    const { onOpenJob } = renderDetail({
      jobs: [
        { job_id: 1, pipeline: "ci.yml", name: "a" },
        { job_id: 2, pipeline: "ci.yml", name: "b" },
      ],
    });
    const first = screen.getByRole("button", { name: "ci.yml · a" });
    const second = screen.getByRole("button", { name: "ci.yml · b" });
    expect(first).toBeInTheDocument();

    // The separator belongs to the second entry, not between the buttons.
    expect(within(second.parentElement as HTMLElement).getByText("·")).toBeInTheDocument();

    await userEvent.click(second);
    expect(onOpenJob).toHaveBeenCalledWith(2);
  });

  it("hides the latest-failure panel when details and message are both empty", () => {
    renderDetail({
      executions: [{ ...history.executions[0], details: "", message: "" }],
    });
    expect(screen.queryByText(/Latest failure/)).toBeNull();
  });

  it("hides the latest-failure panel when nothing failed", () => {
    const { container } = renderDetail({ executions: [{ ...history.executions[1] }] });
    expect(screen.queryByText(/Latest failure/)).toBeNull();
    expect(hoverTargets(container)).toHaveLength(1);
  });

  it("falls back to the message when the latest failure has no details", () => {
    renderDetail({
      executions: [{ ...history.executions[0], details: "", message: "boom msg" }],
    });
    expect(screen.getByText("boom msg", { selector: "pre" })).toBeInTheDocument();
  });
});
