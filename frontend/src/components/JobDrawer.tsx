import { useEffect } from "react";
import type { JobExecution, JobHistory } from "../api";
import { breakable } from "../breakable";
import { IssueLink } from "./IssueLink";
import { StatusMark } from "./StatusMark";

function fmtWhen(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Outcome pill: explained failures get their own look and list their Tests. */
function OutcomeCell({ e }: { e: JobExecution }) {
  if (e.outcome === "explained") {
    return <span className="inline-flex items-center gap-1.5 text-text-2">explained</span>;
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-text-2">
      <StatusMark status={e.outcome} size={9} />
      {e.outcome}
    </span>
  );
}

/** Slide-over panel for one Job. */
export function JobDrawer({
  jobId,
  history,
  onClose,
  onOpenTest,
}: {
  jobId: number;
  history: JobHistory | null;
  onClose: () => void;
  onOpenTest: (testId: number) => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const loaded = history != null && history.job.id === jobId;
  const job = history?.job;
  return (
    <>
      <div className="fixed inset-0 z-20 bg-scrim" onClick={onClose} aria-hidden />
      <aside
        className="fixed inset-y-0 right-0 z-21 w-full max-w-190 animate-slide-in overflow-y-auto border-l border-line bg-surface px-5 pt-4 pb-8 shadow-2xl"
        role="dialog"
        aria-modal="true"
        aria-label="Job detail"
      >
        <div className="mb-2 flex items-start justify-between gap-3">
          <div className="text-xs text-link [overflow-wrap:anywhere]">
            {job ? breakable(`${job.repo} · ${job.pipeline}`) : "Loading…"}
          </div>
          <button
            type="button"
            className="btn grid size-8 place-items-center p-0 text-lg leading-none"
            onClick={onClose}
            aria-label="Close"
          >
            ×
          </button>
        </div>
        {loaded && job && (
          <>
            <h2 className="text-lg font-semibold leading-snug [overflow-wrap:anywhere]">
              {breakable(job.name)}
            </h2>
            <div className="mb-3 text-xs text-muted">
              {job.github_issue_number != null && (
                <>
                  <IssueLink
                    number={job.github_issue_number}
                    url={job.github_issue_url}
                    className="text-link underline-offset-2 hover:underline"
                  >
                    issue #{job.github_issue_number}
                  </IssueLink>
                </>
              )}
            </div>
            <div className="mb-4 rounded-lg border border-line bg-surface-2 px-3 py-2 text-xs text-text-2">
              {history.unexplained_failures} unexplained failure
              {history.unexplained_failures === 1 ? "" : "s"} · {history.explained_failures}{" "}
              explained by tests (last {history.executions.length} executions)
            </div>

            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="border-b border-line text-left text-muted">
                  <th className="py-1.5 pr-3 font-medium">Outcome</th>
                  <th className="py-1.5 pr-3 font-medium">Commit</th>
                  <th className="py-1.5 pr-3 font-medium">Branch</th>
                  <th className="py-1.5 pr-3 font-medium">Runner</th>
                  <th className="py-1.5 pr-3 font-medium">When</th>
                </tr>
              </thead>
              <tbody>
                {history.executions.map((e) => (
                  <tr
                    key={e.id}
                    className="border-b border-line tabular-nums whitespace-nowrap [&>td]:py-2 [&>td]:pr-3"
                  >
                    <td className="min-w-32">
                      <div className="flex items-center gap-1.5 whitespace-nowrap">
                        <OutcomeCell e={e} />
                        <span className="text-muted">attempt {e.ci_run_attempt}</span>
                      </div>
                      {e.outcome === "explained" && e.explained_by.length > 0 && (
                        <div className="mt-1 flex flex-wrap gap-1 whitespace-normal">
                          {e.explained_by.map((t) => (
                            <button
                              key={t.test_id}
                              type="button"
                              onClick={() => onOpenTest(t.test_id)}
                              className="rounded border border-line bg-surface px-1.5 py-0.5 text-xs text-link hover:underline"
                              title="Open this Test"
                            >
                              {t.project} · {t.name} ({t.status})
                            </button>
                          ))}
                        </div>
                      )}
                    </td>
                    <td className="font-mono">{e.commit_sha.slice(0, 10)}</td>
                    <td>{e.branch}</td>
                    <td className="whitespace-nowrap">
                      <div>{e.runner_name || "—"}</div>
                      {e.runner_labels.length > 0 && (
                        <div className="text-muted">{e.runner_labels.join(", ")}</div>
                      )}
                    </td>
                    <td>
                      {e.url ? (
                        <a
                          className="text-link underline-offset-2 hover:underline"
                          href={e.url}
                          target="_blank"
                          rel="noreferrer"
                        >
                          {fmtWhen(e.created_at)}
                        </a>
                      ) : (
                        fmtWhen(e.created_at)
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </aside>
    </>
  );
}
