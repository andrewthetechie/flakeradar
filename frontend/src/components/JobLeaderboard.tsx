import type { Job } from "../api";
import { breakable } from "../breakable";
import { FlakinessCell, ProvenCell, StatusIssueCell } from "./resultCells";
import { SummaryBadge } from "./SummaryBadge";

function jobLabel(job: Job, repo: string | null): string | null {
  // Show the Repo when the view spans repos, otherwise the Pipeline is enough.
  if (!repo) return `${job.repo} · ${job.pipeline}`;
  return job.pipeline;
}

export function JobLeaderboard({
  jobs,
  repo,
  showStable,
  search = "",
  selectedId,
  onSelect,
}: {
  jobs: Job[];
  repo: string | null;
  showStable: boolean;
  search?: string;
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  if (jobs.length === 0) {
    return (
      <div className="px-4 py-10 text-center text-text-2">
        {search ? (
          <>No jobs match.</>
        ) : showStable ? (
          <>
            No job data yet. Copy the{" "}
            <code className="font-mono text-text">samples/flakeradar-jobs.yml</code> reporter
            workflow — see the README.
          </>
        ) : (
          <>No flaky or suspect jobs in this view. Tick “Show stable jobs” to list every job.</>
        )}
      </div>
    );
  }
  const th = "px-4 py-2 text-left text-xs font-medium text-muted";
  return (
    <table className="leaderboard w-full border-collapse">
      <thead>
        <tr className="border-b border-line">
          <th className={th}>Job</th>
          <th className={th}>Flakiness</th>
          <th className={`col-proof ${th}`}>Proof</th>
          <th className={`col-status ${th}`}>Last status</th>
          <th className={th}>Last seen</th>
        </tr>
      </thead>
      <tbody>
        {jobs.map((j) => {
          const label = jobLabel(j, repo);
          const selected = j.id === selectedId;
          return (
            <tr
              key={j.id}
              className={`cursor-pointer border-b border-line last:border-b-0 hover:bg-surface-2 ${
                selected ? "bg-surface-2 shadow-[inset_3px_0_0_var(--signal)]" : ""
              }`}
              onClick={() => onSelect(j.id)}
            >
              <td className="px-4 py-2.5 align-top">
                {label && <div className="text-xs text-link [overflow-wrap:anywhere]">{label}</div>}
                <div className="font-semibold [overflow-wrap:anywhere]">
                  {breakable(j.name)}
                  {j.summary && <SummaryBadge className="ml-1.5" />}
                </div>
              </td>
              <td className="px-4 py-2.5 align-top">
                <FlakinessCell score={j.flakiness_score} tier={j.tier} trend={j.trend} />
              </td>
              <ProvenCell count={j.confirmed_flake_count} />
              <StatusIssueCell
                status={j.last_status}
                issueNumber={j.github_issue_number}
                issueUrl={j.github_issue_url}
                issueClosed={j.github_issue_closed}
              />
              <td className="px-4 py-2.5 align-top text-xs whitespace-nowrap text-text-2">
                {new Date(j.last_seen_at).toLocaleDateString(undefined, {
                  month: "short",
                  day: "numeric",
                })}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
