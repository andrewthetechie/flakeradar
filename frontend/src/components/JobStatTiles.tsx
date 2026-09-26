import type { JobSummary } from "../api";

export function JobStatTiles({ summary }: { summary: JobSummary }) {
  const tiles = [
    { label: "Tracked jobs", value: summary.total_jobs, alert: false },
    {
      label: `Flaky (score ≥ ${summary.flake_threshold})`,
      value: summary.flaky_jobs,
      alert: summary.flaky_jobs > 0,
    },
    { label: "Suspect (score > 0)", value: summary.suspect_jobs, alert: false },
    {
      label: "Proven flaky (same-commit flip)",
      value: summary.confirmed_flaky_jobs,
      alert: summary.confirmed_flaky_jobs > 0,
    },
    { label: "Job executions recorded", value: summary.total_job_executions, alert: false },
    { label: "", value: 0, alert: false },
  ].filter((t) => t.label !== "");
  return (
    <div className="mb-6 grid grid-cols-2 gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-3 lg:grid-cols-5">
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
