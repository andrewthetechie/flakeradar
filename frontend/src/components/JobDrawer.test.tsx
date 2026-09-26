import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { JobHistory } from "../api";
import { JobDrawer } from "./JobDrawer";

const history: JobHistory = {
  job: {
    id: 3,
    repo: "acme/app",
    provider: "github",
    pipeline: ".github/workflows/ci.yml",
    name: "e2e (ubuntu-latest)",
    flakiness_score: 0.31,
    tier: "flaky",
    confirmed_flake_count: 0,
    clean_streak: 4,
    trend: null,
    last_status: "failed",
    last_seen_at: "2026-09-25T00:00:00Z",
    github_issue_number: null,
    github_issue_url: null,
  },
  unexplained_failures: 1,
  explained_failures: 1,
  executions: [
    {
      id: 2,
      status: "failed",
      outcome: "explained",
      commit_sha: "bbb222",
      branch: "feat",
      ci_run_id: "2",
      ci_run_attempt: 1,
      ci_job_id: "J2",
      url: "https://github.com/acme/app/actions/runs/2/jobs/J2",
      runner_name: "runner-1",
      runner_labels: ["ubuntu-latest", "self-hosted"],
      started_at: "2026-09-25T00:00:00Z",
      completed_at: "2026-09-25T00:01:00Z",
      created_at: "2026-09-25T00:01:00Z",
      explained_by: [
        {
          test_id: 9,
          project: "backend",
          classname: "tests",
          name: "test_login",
          status: "failed",
        },
      ],
    },
    {
      id: 1,
      status: "failed",
      outcome: "failed",
      commit_sha: "aaa111",
      branch: "main",
      ci_run_id: "1",
      ci_run_attempt: 1,
      ci_job_id: "J1",
      url: "",
      runner_name: "",
      runner_labels: [],
      started_at: null,
      completed_at: null,
      created_at: "2026-09-25T00:00:00Z",
      explained_by: [],
    },
  ],
  score_history: [],
};

function renderDrawer(overrides: Partial<JobHistory> = {}, onClose = vi.fn()) {
  const onOpenTest = vi.fn();
  render(
    <JobDrawer
      jobId={3}
      history={{ ...history, ...overrides }}
      onClose={onClose}
      onOpenTest={onOpenTest}
    />,
  );
  return { onClose, onOpenTest };
}

describe("JobDrawer", () => {
  it("shows header, summary counts and outcome rows", () => {
    renderDrawer();
    expect(screen.getByRole("dialog", { name: "Job detail" })).toBeInTheDocument();
    expect(screen.getByText(/acme\/app · \.github\/workflows\/ci\.yml/)).toBeInTheDocument();
    expect(
      screen.getByText(
        "1 unexplained failure · 1 explained by tests (last 2 executions) · clean streak 4",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("No score history yet.")).toBeInTheDocument();
    expect(screen.getAllByText("attempt 1").length).toBe(2);
    expect(screen.getByText("runner-1")).toBeInTheDocument();
  });

  it("lists explaining tests and opens the Test drawer on click", async () => {
    const { onOpenTest } = renderDrawer();
    expect(screen.getByText("backend · test_login (failed)")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "backend · test_login (failed)" }));
    expect(onOpenTest).toHaveBeenCalledWith(9);
  });

  it("renders the CI link when present", () => {
    renderDrawer();
    const link = screen.getByRole("link");
    expect(link).toHaveAttribute("href", "https://github.com/acme/app/actions/runs/2/jobs/J2");
  });

  it("closes on Escape and the close button", async () => {
    const { onClose } = renderDrawer();
    fireEvent.keyDown(window, { key: "Escape" });
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
