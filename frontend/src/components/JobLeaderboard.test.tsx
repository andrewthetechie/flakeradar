import { render, screen } from "@testing-library/react";
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
    last_status: "failed",
    last_seen_at: "2026-09-25T00:00:00Z",
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
});
