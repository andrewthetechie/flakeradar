import type { JobSummary } from "../api";
import { StatStrip } from "./StatTiles";

export function JobStatTiles({ summary }: { summary: JobSummary }) {
  return (
    <StatStrip
      tiles={[
        { label: "Tracked jobs", value: summary.total_jobs },
        {
          label: `Flaky (score ≥ ${summary.flake_threshold})`,
          value: summary.flaky_jobs,
          alert: summary.flaky_jobs > 0,
        },
        { label: "Suspect (score > 0)", value: summary.suspect_jobs },
        {
          label: "Proven flaky",
          value: summary.confirmed_flaky_jobs,
          alert: summary.confirmed_flaky_jobs > 0,
        },
        { label: "Job executions recorded", value: summary.total_job_executions },
      ]}
    />
  );
}
