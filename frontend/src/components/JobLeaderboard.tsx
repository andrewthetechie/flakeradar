import type { Job } from "../api";
import { breakable } from "../breakable";
import { formatScore } from "../format";
import { IssueLink } from "./IssueLink";
import { StatusMark } from "./StatusMark";

function scoreColor(score: number): string {
  if (score >= 0.75) return "var(--seq-650)";
  if (score >= 0.5) return "var(--seq-550)";
  if (score >= 0.25) return "var(--seq-400)";
  return "var(--seq-250)";
}

const TIER_CLASS: Record<Job["tier"], string> = {
  flaky: "font-semibold text-signal",
  suspect: "text-text-2",
  stable: "text-muted",
};

function jobLabel(job: Job, repo: string | null): string | null {
  // Show the Repo when the view spans repos, otherwise the Pipeline is enough.
  if (!repo) return `${job.repo} · ${job.pipeline}`;
  return job.pipeline;
}

export function JobLeaderboard({
  jobs,
  repo,
  showStable,
  selectedId,
  onSelect,
}: {
  jobs: Job[];
  repo: string | null;
  showStable: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  if (jobs.length === 0) {
    return (
      <div className="px-4 py-10 text-center text-text-2">
        {showStable ? (
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
                <div className="font-semibold [overflow-wrap:anywhere]">{breakable(j.name)}</div>
              </td>
              <td className="px-4 py-2.5 align-top">
                <div className="flex min-w-18 items-center gap-2 sm:min-w-32">
                  <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-line">
                    <div
                      className="h-full rounded-full"
                      style={{
                        width: `${Math.max(j.flakiness_score * 100, j.flakiness_score > 0 ? 4 : 0)}%`,
                        background: scoreColor(j.flakiness_score),
                      }}
                    />
                  </div>
                  <span className="w-9 text-right tabular-nums text-text-2">
                    {formatScore(j.flakiness_score)}
                  </span>
                </div>
                <span className={`mt-1 inline-block text-xs ${TIER_CLASS[j.tier]}`}>{j.tier}</span>
              </td>
              <td className="col-proof px-4 py-2.5 align-top text-xs whitespace-nowrap">
                {j.confirmed_flake_count > 0 ? (
                  <span className="text-signal" title="Failed and passed on the same commit">
                    ⚠ {j.confirmed_flake_count}× proven
                  </span>
                ) : (
                  <span className="text-muted">—</span>
                )}
              </td>
              <td className="col-status px-4 py-2.5 align-top">
                <span className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-text-2">
                  <StatusMark status={j.last_status} /> {j.last_status}
                  <IssueLink
                    number={j.github_issue_number}
                    url={j.github_issue_url}
                    onClick={(e) => e.stopPropagation()}
                    className="rounded-full border border-line px-2 text-link hover:underline"
                    title="GitHub issue filed by FlakeRadar"
                  >
                    #{j.github_issue_number}
                  </IssueLink>
                </span>
              </td>
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
