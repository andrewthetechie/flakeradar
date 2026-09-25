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
  return (
    <div className="tiles">
      {tiles.map((t) => (
        <div className="tile" key={t.label}>
          <div className="label">{t.label}</div>
          <div className={t.alert ? "value alert" : "value"}>{t.value}</div>
        </div>
      ))}
    </div>
  );
}
