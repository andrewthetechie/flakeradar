import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { History } from "../api";
import { TestDrawer } from "./TestDrawer";

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
    last_status: "passed",
    last_seen_at: "2026-09-25T00:00:00Z",
    quarantined: false,
    quarantined_at: null,
    github_issue_number: null,
  },
  location: {
    path: "frontend/src/app.test.ts",
    line: 12,
    url: "https://github.com/andrewthetechie/writers-app/blob/aaa111/frontend/src/app.test.ts#L12",
  },
  last_failing_sha: "aaa111",
  last_failing_branch: "main",
  executions: [
    {
      id: 2,
      status: "passed",
      duration: 0.1,
      message: "",
      details: "",
      created_at: "2026-09-25T01:00:00Z",
      commit_sha: "bbb222",
      branch: "feat",
      ci_run_id: "2",
    },
    {
      id: 1,
      status: "failed",
      duration: 0.1,
      message: "expected 3",
      details: "Traceback\n  at src/app.test.ts:14",
      created_at: "2026-09-25T00:00:00Z",
      commit_sha: "aaa111",
      branch: "main",
      ci_run_id: "1",
    },
  ],
};

function renderDrawer(overrides: Partial<History> = {}, onClose = vi.fn(), onQ = vi.fn()) {
  render(
    <TestDrawer
      testId={7}
      history={{ ...history, ...overrides }}
      onClose={onClose}
      onToggleQuarantine={onQ}
    />,
  );
  return { onClose, onQ };
}

describe("TestDrawer", () => {
  it("shows breadcrumb, permalink and the latest failure details", () => {
    renderDrawer();
    expect(screen.getByRole("dialog", { name: "Test detail" })).toBeInTheDocument();
    expect(screen.getByText(/andrewthetechie\/writers-app \/ frontend/)).toBeInTheDocument();
    const link = screen.getByRole("link");
    expect(link).toHaveAttribute("href", history.location!.url);
    expect(link).toHaveTextContent("frontend/src/app.test.ts:12");
    expect(screen.getByText(/Traceback/)).toHaveTextContent("Traceback at src/app.test.ts:14");
  });

  it("says when the runner reported no location", () => {
    renderDrawer({ location: null });
    expect(screen.getByText("Location not reported by the test runner")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("closes on Escape and on the close button", async () => {
    const { onClose } = renderDrawer();
    fireEvent.keyDown(window, { key: "Escape" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("toggles quarantine for the shown test", async () => {
    const { onQ } = renderDrawer();
    await userEvent.click(screen.getByRole("button", { name: "Quarantine" }));
    expect(onQ).toHaveBeenCalledWith(history.test);
  });

  it("shows a loading state until the right test's history arrives", () => {
    render(
      <TestDrawer testId={99} history={history} onClose={() => {}} onToggleQuarantine={() => {}} />,
    );
    expect(screen.getByText("Loading…")).toBeInTheDocument();
  });
});
