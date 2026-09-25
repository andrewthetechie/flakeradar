/** Two-decimal score that never rounds UP across the flake threshold:
 *  0.2975 shows as 0.29 (suspect), not 0.30 next to a "suspect" tier.
 *  The epsilon absorbs float error (0.29 * 100 = 28.999…). */
export function formatScore(score: number): string {
  return (Math.floor(score * 100 + 1e-9) / 100).toFixed(2);
}
