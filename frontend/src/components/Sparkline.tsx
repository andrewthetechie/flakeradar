import type { ScorePoint } from "../api";
import { formatScore } from "../format";

const PAD = 2;
const DAY_MS = 86_400_000;

/** SVG path for a step line of daily scores. x is placed by calendar day (gaps carry
 *  the last value forward); y maps score 0..1 to bottom..top. Coordinates are rounded
 *  to 2 decimals. */
export function sparklinePath(points: ScorePoint[], width: number, height: number): string {
  const t0 = Date.parse(`${points[0].day}T00:00:00Z`);
  const idx = points.map((p) => (Date.parse(`${p.day}T00:00:00Z`) - t0) / DAY_MS);
  const span = Math.max(idx[idx.length - 1], 1);
  const r = (n: number) => Math.round(n * 100) / 100;
  const x = (i: number) => r(PAD + (i / span) * (width - 2 * PAD));
  const y = (s: number) => r(PAD + (1 - s) * (height - 2 * PAD));
  let d = `M${x(0)},${y(points[0].flakiness_score)}`;
  for (let k = 1; k < points.length; k++) d += ` H${x(idx[k])} V${y(points[k].flakiness_score)}`;
  if (points.length === 1) d += ` H${x(span)}`;
  return d;
}

export function Sparkline({
  points,
  label,
  width = 240,
  height = 40,
}: {
  points: ScorePoint[];
  label: string;
  width?: number;
  height?: number;
}) {
  if (points.length === 0) return <div className="text-xs text-muted">No score history yet.</div>;
  const first = points[0].flakiness_score;
  const last = points[points.length - 1].flakiness_score;
  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={`${label}, ${points.length} days recorded: ${formatScore(first)} to ${formatScore(last)}`}
      style={{ display: "block", maxWidth: "100%" }}
    >
      <path
        d={sparklinePath(points, width, height)}
        fill="none"
        stroke="var(--seq-400)"
        strokeWidth={1.5}
      />
    </svg>
  );
}
