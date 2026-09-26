import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { JobStatTiles } from "./JobStatTiles";
import { StatTiles } from "./StatTiles";

describe("StatTiles", () => {
  it("shows one tile per Test statistic", () => {
    render(
      <StatTiles
        summary={{
          total_tests: 120,
          flaky_tests: 3,
          suspect_tests: 9,
          confirmed_flaky_tests: 2,
          total_runs: 40,
          total_executions: 4800,
          flake_threshold: 0.3,
        }}
      />,
    );
    expect(screen.getAllByTestId("stat-tile")).toHaveLength(6);
    expect(screen.getByText("Tracked tests")).toBeInTheDocument();
    expect(screen.getByText("Proven flaky")).toBeInTheDocument();
  });

  it("shows one tile per Job statistic, with no empty tile", () => {
    render(
      <JobStatTiles
        summary={{
          total_jobs: 12,
          flaky_jobs: 1,
          suspect_jobs: 2,
          confirmed_flaky_jobs: 1,
          total_job_executions: 300,
          flake_threshold: 0.3,
        }}
      />,
    );
    const tiles = screen.getAllByTestId("stat-tile");
    expect(tiles).toHaveLength(5);
    expect(tiles.every((t) => t.textContent !== "")).toBe(true);
    expect(screen.getByText("Tracked jobs")).toBeInTheDocument();
    expect(screen.getByText("Job executions recorded")).toBeInTheDocument();
  });
});
