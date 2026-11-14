import type { History, TestCase } from "../api";
import { breakable } from "../breakable";
import { IssueLink } from "./IssueLink";
import { Overlay } from "./Overlay";
import { BrokenPill } from "./resultCells";
import { TestDetail } from "./TestDetail";

/** Whole days from now until an ISO timestamp, rounding a partial day up. */
function daysUntil(iso: string): number {
  return Math.ceil((Date.parse(iso) - Date.now()) / 86_400_000);
}

/** Slide-over panel for one Test. The leaderboard keeps the full width. */
export function TestDrawer({
  testId,
  history,
  onClose,
  onToggleQuarantine,
  onOpenJob,
  onOpenCiRun,
}: {
  testId: number;
  history: History | null;
  onClose: () => void;
  onToggleQuarantine: (t: TestCase) => void;
  onOpenJob: (jobId: number) => void;
  onOpenCiRun: (repo: string, ciRun: string, attempt: number | null) => void;
}) {
  const loaded = history != null && history.test.id === testId;
  const loc = history?.location;
  return (
    <Overlay
      variant="drawer"
      label="Test detail"
      onClose={onClose}
      breadcrumb={loaded ? breakable(`${history.test.repo} / ${history.test.project}`) : "Loading…"}
    >
      {loaded && (
        <>
          <h2 className="text-lg font-semibold leading-snug [overflow-wrap:anywhere]">
            {history.test.broken && <BrokenPill />}
            {breakable(history.test.name)}
          </h2>
          <div className="mb-3 text-xs text-muted [overflow-wrap:anywhere]">
            {breakable(history.test.classname || history.test.suite)}
            {history.test.github_issue_number != null && (
              <>
                {" "}
                ·{" "}
                <IssueLink
                  number={history.test.github_issue_number}
                  url={history.test.github_issue_url}
                  closed={history.test.github_issue_closed}
                  className="text-link underline-offset-2 hover:underline"
                >
                  issue #{history.test.github_issue_number}
                </IssueLink>
              </>
            )}
          </div>
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0 font-mono text-xs [overflow-wrap:anywhere]">
              {loc == null ? (
                <span className="font-sans text-muted">
                  Location not reported by the test runner
                </span>
              ) : loc.url ? (
                <a
                  className="text-link underline-offset-2 hover:underline"
                  href={loc.url}
                  target="_blank"
                  rel="noreferrer"
                >
                  {breakable(loc.path)}
                  {loc.line != null && `:${loc.line}`}
                </a>
              ) : (
                <span>
                  {breakable(loc.path)}
                  {loc.line != null && `:${loc.line}`}
                </span>
              )}
            </div>
            <button type="button" className="btn" onClick={() => onToggleQuarantine(history.test)}>
              {history.test.quarantined ? "Un-quarantine" : "Quarantine"}
            </button>
          </div>
          {history.test.quarantined && (
            <div className="-mt-2 mb-4 text-xs text-muted [overflow-wrap:anywhere]">
              {history.test.quarantine_reason != null && (
                <div>Reason: {breakable(history.test.quarantine_reason)}</div>
              )}
              {history.test.quarantine_expires_at != null && (
                <div>expires in {daysUntil(history.test.quarantine_expires_at)} days</div>
              )}
            </div>
          )}
          <TestDetail history={history} onOpenJob={onOpenJob} onOpenCiRun={onOpenCiRun} />
        </>
      )}
    </Overlay>
  );
}
