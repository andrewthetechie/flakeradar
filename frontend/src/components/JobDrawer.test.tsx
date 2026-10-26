import { fireEvent, render, screen, within } from "@testing-library/react";
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
    summary: false,
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
      explained_by_jobs: [],
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
      explained_by_jobs: [],
    },
  ],
  score_history: [],
  runners: [
    {
      labels: ["self-hosted", "gpu"],
      runner_name: null,
      executions: 3,
      unexplained_failures: 2,
    },
    {
      labels: ["ubuntu-latest"],
      runner_name: null,
      executions: 3,
      unexplained_failures: 0,
    },
  ],
};

function renderDrawer(overrides: Partial<JobHistory> = {}, onClose = vi.fn()) {
  const onOpenTest = vi.fn();
  const onOpenJob = vi.fn();
  const onOpenCiRun = vi.fn();
  render(
    <JobDrawer
      jobId={3}
      history={{ ...history, ...overrides }}
      onClose={onClose}
      onOpenTest={onOpenTest}
      onOpenJob={onOpenJob}
      onOpenCiRun={onOpenCiRun}
    />,
  );
  return { onClose, onOpenTest, onOpenJob, onOpenCiRun };
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

  it("renders the failures-by-runner table", () => {
    renderDrawer();
    const table = screen.getByRole("table", { name: "Failures by runner" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(3); // header + 2 groups
    const group = within(rows[1]);
    expect(group.getByText("self-hosted, gpu")).toBeInTheDocument();
    expect(group.getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "self-hosted, gpu",
      "3",
      "2",
    ]);
    expect(within(rows[2]).getByText("ubuntu-latest")).toBeInTheDocument();
  });

  it("shows the runner name instead of labels when a group is named", () => {
    renderDrawer({
      runners: [
        {
          labels: ["self-hosted"],
          runner_name: "gpu-box-1",
          executions: 4,
          unexplained_failures: 1,
        },
      ],
    });
    const table = screen.getByRole("table", { name: "Failures by runner" });
    expect(within(table).getByText("gpu-box-1")).toBeInTheDocument();
    expect(within(table).queryByText("self-hosted")).toBeNull();
  });

  it("omits the failures-by-runner table when there are no groups", () => {
    renderDrawer({ runners: [] });
    expect(screen.queryByText("Failures by runner")).toBeNull();
  });

  it("lists explaining tests and opens the Test drawer on click", async () => {
    const { onOpenTest } = renderDrawer();
    expect(screen.getByText("backend · test_login (failed)")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "backend · test_login (failed)" }));
    expect(onOpenTest).toHaveBeenCalledWith(9);
  });

  it("opens the CI run view from an execution row, with its attempt", async () => {
    const { onOpenCiRun } = renderDrawer();
    await userEvent.click(screen.getByRole("button", { name: "run 2" }));
    expect(onOpenCiRun).toHaveBeenCalledWith("acme/app", "2", 1);
  });

  it("shows the summary badge and buttons for the explaining sibling Jobs", async () => {
    const { onOpenJob } = renderDrawer({
      job: { ...history.job, summary: true, name: "alls-green" },
      executions: [
        {
          ...history.executions[0],
          explained_by: [],
          explained_by_jobs: [
            {
              job_id: 7,
              pipeline: ".github/workflows/ci.yml",
              name: "test (ubuntu)",
              url: "https://github.com/acme/app/actions/runs/1/jobs/2",
            },
          ],
        },
      ],
    });
    expect(
      screen.getByRole("button", { name: ".github/workflows/ci.yml · test (ubuntu)" }),
    ).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: ".github/workflows/ci.yml · test (ubuntu)" }),
    );
    expect(onOpenJob).toHaveBeenCalledWith(7);
    expect(
      screen.getByTitle("Runs no tests; explained by a failed job in the same CI run attempt"),
    ).toBeInTheDocument();
  });

  it("shows no badge and no sibling Job buttons for a non-summary Job", () => {
    renderDrawer();
    expect(
      screen.queryByTitle("Runs no tests; explained by a failed job in the same CI run attempt"),
    ).toBeNull();
    expect(screen.queryAllByTitle("Open this Job")).toHaveLength(0);
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
