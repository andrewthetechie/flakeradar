import type { Trend } from "../api";

const TREND: Record<Trend, { glyph: string; cls: string; title: string }> = {
  worsening: {
    glyph: "↗",
    cls: "text-critical",
    title: "Worsening: score up by 0.05 or more versus 14 days ago",
  },
  improving: {
    glyph: "↘",
    cls: "text-good",
    title: "Improving: score down by 0.05 or more versus 14 days ago",
  },
  steady: { glyph: "→", cls: "text-muted", title: "Steady versus 14 days ago" },
};

export function TrendMark({ trend }: { trend: Trend | null }) {
  if (!trend) return null;
  const t = TREND[trend];
  return (
    <span className={`ml-1 text-xs ${t.cls}`} title={t.title} aria-label={t.title}>
      {t.glyph}
    </span>
  );
}
