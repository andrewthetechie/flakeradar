import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { JobStatTiles } from "./JobStatTiles";
import { StatStrip, StatTiles } from "./StatTiles";

describe("StatTiles", () => {
  it("shows one tile per Test statistic", () => {
    render(
      <StatTiles
        summary={{
          total_tests: 120,
          flaky_tests: 3,
          suspect_tests: 9,
          confirmed_flaky_tests: 2,
          new_flaky_7d: 4,
          total_runs: 40,
          executions_30d: 4800,
          failures_30d: 21,
          flake_threshold: 0.3,
          category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
        }}
      />,
    );
    expect(screen.getAllByTestId("stat-tile")).toHaveLength(8);
    expect(screen.getByText("Tracked tests")).toBeInTheDocument();
    expect(screen.getByText("Proven flaky")).toBeInTheDocument();
    expect(screen.getByText("New in 7 days")).toBeInTheDocument();
    expect(screen.getByText("New in 7 days").nextElementSibling).toHaveTextContent("4");
    expect(screen.getByText("Executions (30 days)")).toBeInTheDocument();
    expect(screen.getByText("Executions (30 days)").nextElementSibling).toHaveTextContent("4,800");
    expect(screen.getByText("Failures (30 days)")).toBeInTheDocument();
    expect(screen.getByText("Failures (30 days)").nextElementSibling).toHaveTextContent("21");
  });

  it("shows one tile per Job statistic, with no empty tile", () => {
    render(
      <JobStatTiles
        summary={{
          total_jobs: 12,
          flaky_jobs: 1,
          suspect_jobs: 2,
          confirmed_flaky_jobs: 1,
          new_flaky_7d: 2,
          job_executions_30d: 300,
          job_failures_30d: 5,
          flake_threshold: 0.3,
          pipelines: [],
        }}
      />,
    );
    const tiles = screen.getAllByTestId("stat-tile");
    expect(tiles).toHaveLength(7);
    expect(tiles.every((t) => t.textContent !== "")).toBe(true);
    expect(screen.getByText("Tracked jobs")).toBeInTheDocument();
    expect(screen.getByText("New in 7 days")).toBeInTheDocument();
    expect(screen.getByText("Job executions (30 days)")).toBeInTheDocument();
    expect(screen.getByText("Job executions (30 days)").nextElementSibling).toHaveTextContent(
      "300",
    );
    expect(screen.getByText("Job failures (30 days)")).toBeInTheDocument();
    expect(screen.getByText("Job failures (30 days)").nextElementSibling).toHaveTextContent("5");
  });

  it("falls back to six columns for an unusual tile count", () => {
    const { container } = render(
      <StatStrip
        tiles={[
          { label: "A", value: 1 },
          { label: "B", value: 2 },
          { label: "C", value: 3 },
        ]}
      />,
    );
    expect(container.firstElementChild?.className).toContain("lg:grid-cols-6");
  });
});
