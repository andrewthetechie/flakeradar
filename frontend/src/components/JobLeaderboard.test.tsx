import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Job } from "../api";
import { JobLeaderboard } from "./JobLeaderboard";

function job(overrides: Partial<Job> = {}): Job {
  return {
    id: 1,
    repo: "acme/app",
    provider: "github",
    pipeline: ".github/workflows/ci.yml",
    name: "test (ubuntu, 3.12)",
    flakiness_score: 0.6,
    tier: "flaky",
    confirmed_flake_count: 2,
    clean_streak: 0,
    trend: null,
    last_status: "failed",
    last_seen_at: "2026-09-25T00:00:00Z",
    summary: false,
    github_issue_number: null,
    github_issue_url: null,
    ...overrides,
  };
}

describe("JobLeaderboard", () => {
  it("renders jobs with pipeline, score, tier, proof and status", () => {
    render(
      <JobLeaderboard
        jobs={[job()]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText(".github/workflows/ci.yml")).toBeInTheDocument();
    expect(screen.getByText("test (ubuntu, 3.12)")).toBeInTheDocument();
    expect(screen.getByText("flaky")).toBeInTheDocument();
    expect(screen.getByText(/2× proven/)).toBeInTheDocument();
    expect(screen.getByText("failed")).toBeInTheDocument();
  });

  it("shows the repo when no repo is selected", () => {
    render(
      <JobLeaderboard
        jobs={[job()]}
        repo={null}
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("acme/app · .github/workflows/ci.yml")).toBeInTheDocument();
  });

  it("selects a job on click", () => {
    const onSelect = vi.fn();
    render(
      <JobLeaderboard
        jobs={[job()]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={onSelect}
      />,
    );
    screen.getByText("test (ubuntu, 3.12)").click();
    expect(onSelect).toHaveBeenCalledWith(1);
  });

  it("shows the empty state when there are no jobs", () => {
    render(
      <JobLeaderboard
        jobs={[]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText(/No flaky or suspect jobs in this view/)).toBeInTheDocument();
  });

  it("says no jobs match when a search returns nothing", () => {
    render(
      <JobLeaderboard
        jobs={[]}
        repo="acme/app"
        showStable={false}
        search="nope"
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("No jobs match.")).toBeInTheDocument();
    expect(screen.queryByText(/Tick “Show stable jobs”/)).toBeNull();
  });

  it("tells you to copy the reporter workflow when there is no data at all", () => {
    render(
      <JobLeaderboard jobs={[]} repo="acme/app" showStable selectedId={null} onSelect={vi.fn()} />,
    );
    // The sentence is split around the <code> workflow path, so match a fragment.
    expect(screen.getByText(/No job data yet/)).toBeInTheDocument();
  });

  it("marks the selected row", () => {
    const { unmount } = render(
      <JobLeaderboard
        jobs={[job()]}
        repo="acme/app"
        showStable={false}
        selectedId={job().id}
        onSelect={vi.fn()}
      />,
    );
    // The selected-row class is the only signal that a row is selected.
    expect(screen.getByText("test (ubuntu, 3.12)").closest("tr")).toHaveClass("bg-surface-2");
    unmount();
    render(
      <JobLeaderboard
        jobs={[job()]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("test (ubuntu, 3.12)").closest("tr")).not.toHaveClass("bg-surface-2");
  });

  it("badges a summary job", () => {
    render(
      <JobLeaderboard
        jobs={[job({ summary: true })]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("summary")).toBeInTheDocument();
  });

  it("renders a job that has never flipped", () => {
    render(
      <JobLeaderboard
        jobs={[job({ flakiness_score: 0 })]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("0.00")).toBeInTheDocument();
  });

  it("keeps the issue link from selecting the job", () => {
    const onSelect = vi.fn();
    render(
      <JobLeaderboard
        jobs={[
          job({
            github_issue_number: 4,
            github_issue_url: "https://github.com/acme/app/issues/4",
          }),
        ]}
        repo="acme/app"
        showStable={false}
        selectedId={null}
        onSelect={onSelect}
      />,
    );
    fireEvent.click(screen.getByRole("link", { name: "#4" }));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
