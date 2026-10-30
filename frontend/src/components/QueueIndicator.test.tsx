import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReportInfo } from "../api";
import { QueueIndicator } from "./QueueIndicator";

afterEach(() => {
  vi.useRealTimers();
});

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

  it("shows each failed report's age and the dismiss command", async () => {
    // Frozen clock: the report was created 3 days before "now". Fake timers make
    // findByText unusable (see CiRunView.test.tsx), so the row is flushed and read
    // with getByText instead.
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-28T00:00:00Z"));
    const loadFailed = vi.fn().mockResolvedValue([failedReport]);
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    fireEvent.click(screen.getByRole("button"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText("3 days ago")).toBeInTheDocument();
    const dismiss = screen.getByText(/curl -X DELETE/);
    expect(dismiss).toHaveTextContent('-H "X-API-Key: $FLAKERADAR_API_TOKEN"');
    expect(dismiss).toHaveTextContent("/api/reports/<id>");
  });

  it.each([
    ["2026-09-27T23:30:00Z", "1 hour ago"],
    ["2026-09-27T23:00:00Z", "1 hour ago"],
    ["2026-09-27T19:00:00Z", "5 hours ago"],
    ["2026-09-26T18:00:00Z", "1 day ago"],
  ])("ages a report created at %s", async (createdAt, expected) => {
    // Same frozen clock as the test above, 30 minutes to 30 hours of age.
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-28T00:00:00Z"));
    const loadFailed = vi.fn().mockResolvedValue([{ ...failedReport, created_at: createdAt }]);
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    fireEvent.click(screen.getByRole("button"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText(expected)).toBeInTheDocument();
  });

  it("closes the panel on a second click without loading again", async () => {
    const loadFailed = vi.fn().mockResolvedValue([failedReport]);
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    const button = screen.getByRole("button");
    await userEvent.click(button);
    expect(await screen.findByRole("region", { name: "Failed reports" })).toBeInTheDocument();
    await userEvent.click(button);
    expect(screen.queryByRole("region", { name: "Failed reports" })).toBeNull();
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(loadFailed).toHaveBeenCalledOnce();
  });

  it("says so when loading failed reports fails", async () => {
    const loadFailed = vi.fn().mockRejectedValue(new Error("boom"));
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    await userEvent.click(screen.getByRole("button"));
    expect(await screen.findByText("Could not load failed reports (boom).")).toBeInTheDocument();
  });

  it("says so when loading failed reports fails with a non-Error value", async () => {
    const loadFailed = vi.fn().mockRejectedValue("nope");
    render(<QueueIndicator summary={{ pending: 0, failed: 1 }} loadFailed={loadFailed} />);
    await userEvent.click(screen.getByRole("button"));
    expect(await screen.findByText("Could not load failed reports (nope).")).toBeInTheDocument();
  });
});
