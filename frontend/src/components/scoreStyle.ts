import type { Tier } from "../api";

// Sequential blue: stronger = worse (the scale flips per theme in styles.css).
export function scoreColor(score: number): string {
  if (score >= 0.75) return "var(--seq-650)";
  if (score >= 0.5) return "var(--seq-550)";
  if (score >= 0.25) return "var(--seq-400)";
  return "var(--seq-250)";
}

export const TIER_CLASS: Record<Tier, string> = {
  flaky: "font-semibold text-signal",
  suspect: "text-text-2",
  stable: "text-muted",
};
