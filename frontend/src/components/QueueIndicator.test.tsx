import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ReportInfo } from "../api";
import { QueueIndicator } from "./QueueIndicator";

const failedReport: ReportInfo = {
  id: 12,
  kind: "junit",
  repo: "andrewthetechie/writers-app",
  project: "e2e",
  commit_sha: "abcdef1234567",
  branch: "main",
  ci_run_id: "7-1",
  status: "failed",
  error: "ParseError: Not a valid JUnit XML report: syntax error",
  counts: null,
  run_id: null,
  created_at: "2026-09-25T00:00:00Z",
  processed_at: null,
};

describe("QueueIndicator", () => {
  it("renders nothing when the queue is idle", () => {
    const { container } = render(
      <QueueIndicator summary={{ pending: 0, failed: 0 }} loadFailed={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows counts and lists failed reports on click", async () => {
    const loadFailed = vi.fn().mockResolvedValue([failedReport]);
    render(<QueueIndicator summary={{ pending: 3, failed: 1 }} loadFailed={loadFailed} />);
    const button = screen.getByRole("button", { name: "3 pending · 1 failed" });
    await userEvent.click(button);
    expect(loadFailed).toHaveBeenCalledOnce();
    expect(
      await screen.findByText("#12 · andrewthetechie/writers-app / e2e · abcdef1234"),
    ).toBeInTheDocument();
    expect(screen.getByText(/ParseError: Not a valid JUnit XML report/)).toBeInTheDocument();
    expect(button).toHaveAttribute("aria-expanded", "true");
  });

  it("says so when there are no failed reports", async () => {
    render(
      <QueueIndicator
        summary={{ pending: 2, failed: 0 }}
        loadFailed={vi.fn().mockResolvedValue([])}
      />,
    );
    await userEvent.click(screen.getByRole("button"));
    expect(await screen.findByText("No failed reports.")).toBeInTheDocument();
  });

  it("renders a pipeline row when the report has no project", async () => {
    const pipelineReport: ReportInfo = { ...failedReport, id: 13, kind: "pipeline", project: null };
    const loadFailed = vi.fn().mockResolvedValue([pipelineReport]);
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    await userEvent.click(screen.getByRole("button"));
    expect(
      await screen.findByText("#13 · andrewthetechie/writers-app · pipeline · abcdef1234"),
    ).toBeInTheDocument();
  });
});
