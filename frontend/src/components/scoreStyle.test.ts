import { describe, expect, it } from "vitest";
import { scoreColor } from "./scoreStyle";

describe("scoreColor", () => {
  it("maps each tier boundary to its colour, testing both sides of it", () => {
    expect(scoreColor(1)).toBe("var(--seq-650)");
    expect(scoreColor(0.75)).toBe("var(--seq-650)");
    expect(scoreColor(0.74)).toBe("var(--seq-550)");
    expect(scoreColor(0.5)).toBe("var(--seq-550)");
    expect(scoreColor(0.49)).toBe("var(--seq-400)");
    expect(scoreColor(0.25)).toBe("var(--seq-400)");
    expect(scoreColor(0.24)).toBe("var(--seq-250)");
    expect(scoreColor(0)).toBe("var(--seq-250)");
  });
});
