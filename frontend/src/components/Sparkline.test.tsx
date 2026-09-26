import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ScorePoint } from "../api";
import { Sparkline, sparklinePath } from "./Sparkline";

const pt = (day: string, flakiness_score: number): ScorePoint => ({
  day,
  flakiness_score,
  confirmed_flake_count: 0,
  executions: 0,
  failures: 0,
});

describe("sparklinePath", () => {
  it("steps across the full width on a single record", () => {
    expect(sparklinePath([pt("2026-09-01", 0), pt("2026-09-03", 1)], 104, 14)).toBe(
      "M2,12 H102 V2",
    );
  });

  it("draws a flat line for a single point", () => {
    expect(sparklinePath([pt("2026-09-01", 0.5)], 104, 14)).toBe("M2,7 H102");
  });

  it("places x by calendar day, carrying the last value forward", () => {
    expect(
      sparklinePath([pt("2026-09-01", 0.2), pt("2026-09-02", 0.4), pt("2026-09-05", 0.1)], 104, 14),
    ).toBe("M2,10 H27 V8 H102 V11");
  });
});

describe("Sparkline", () => {
  it("shows the empty state when there are no points", () => {
    render(<Sparkline points={[]} label="x" />);
    expect(screen.getByText("No score history yet.")).toBeInTheDocument();
  });
});
