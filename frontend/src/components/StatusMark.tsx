// Status is encoded by SHAPE + color together (validator showed pass-green vs
// fail-red collapse to dE 4.1 under deuteranopia, so color must never be alone):
//   passed  -> filled circle (green)
//   failed  -> filled square (red)
//   error   -> filled diamond (red)
//   skipped -> hollow circle (gray)

const COLORS: Record<string, string> = {
  passed: "var(--status-good)",
  failed: "var(--status-critical)",
  error: "var(--status-critical)",
  skipped: "var(--muted)",
};

export function statusColor(status: string): string {
  return COLORS[status] ?? "var(--muted)";
}

export function StatusMark({ status, size = 10 }: { status: string; size?: number }) {
  const c = statusColor(status);
  const half = size / 2;
  return (
    <svg className="shrink-0" width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden>
      <MarkShape status={status} cx={half} cy={half} r={half - 1} color={c} />
    </svg>
  );
}

// Raw shape for embedding inside a larger SVG (the execution strip).
export function MarkShape({
  status, cx, cy, r, color,
}: { status: string; cx: number; cy: number; r: number; color: string }) {
  if (status === "failed") {
    return <rect x={cx - r} y={cy - r} width={r * 2} height={r * 2} rx={1.5} fill={color} />;
  }
  if (status === "error") {
    const pts = `${cx},${cy - r} ${cx + r},${cy} ${cx},${cy + r} ${cx - r},${cy}`;
    return <polygon points={pts} fill={color} />;
  }
  if (status === "skipped") {
    return <circle cx={cx} cy={cy} r={r - 0.75} fill="none" stroke={color} strokeWidth={1.5} />;
  }
  return <circle cx={cx} cy={cy} r={r} fill={color} />;
}
