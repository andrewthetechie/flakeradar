import type { Summary } from "../api";

export function StatTiles({ summary }: { summary: Summary }) {
  const tiles = [
    { label: "Tracked tests", value: summary.total_tests, alert: false },
    {
      label: `Flaky (score ≥ ${summary.flake_threshold})`,
      value: summary.flaky_tests,
      alert: summary.flaky_tests > 0,
    },
    { label: "Suspect (score > 0)", value: summary.suspect_tests, alert: false },
    {
      label: "Proven flaky (same-commit flip)",
      value: summary.confirmed_flaky_tests,
      alert: summary.confirmed_flaky_tests > 0,
    },
    { label: "CI runs ingested", value: summary.total_runs, alert: false },
    { label: "Executions recorded", value: summary.total_executions, alert: false },
  ];
  // One strip split by hairlines (gap-px over the line color), not six cards.
  return (
    <div className="mb-6 grid grid-cols-2 gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-3 lg:grid-cols-6">
      {tiles.map((t) => (
        <div className="flex flex-col justify-between bg-surface px-4 py-3" key={t.label}>
          <div className="text-xs text-text-2">{t.label}</div>
          <div
            className={`mt-1 text-[22px] font-semibold tabular-nums ${t.alert ? "text-signal" : ""}`}
          >
            {t.value.toLocaleString()}
          </div>
        </div>
      ))}
    </div>
  );
}
