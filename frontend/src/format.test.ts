import { describe, expect, it } from "vitest";
import { formatScore } from "./format";

describe("formatScore", () => {
  it("floors to two decimals so the number agrees with the tier", () => {
    expect(formatScore(0.2975)).toBe("0.29");
    expect(formatScore(0.3)).toBe("0.30");
    expect(formatScore(0.29)).toBe("0.29");
    expect(formatScore(1)).toBe("1.00");
    expect(formatScore(0)).toBe("0.00");
  });
});
