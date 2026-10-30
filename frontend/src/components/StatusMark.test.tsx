import { describe, expect, it } from "vitest";
import { statusColor } from "./StatusMark";

describe("statusColor", () => {
  it("colors a known status", () => {
    expect(statusColor("failed")).toBe("var(--status-critical)");
  });

  it("falls back to muted for an unknown status", () => {
    expect(statusColor("weird")).toBe("var(--muted)");
  });
});
