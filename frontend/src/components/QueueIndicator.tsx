import { useState } from "react";
import type { ReportInfo, ReportSummary } from "../api";

/** "3 pending · 1 failed" — the only place a broken CI upload becomes visible.
 *  Hidden while the queue is empty and nothing has failed. */
export function QueueIndicator({
  summary, loadFailed,
}: {
  summary: ReportSummary | null;
  loadFailed: () => Promise<ReportInfo[]>;
}) {
  const [open, setOpen] = useState(false);
  const [failed, setFailed] = useState<ReportInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!summary || (summary.pending === 0 && summary.failed === 0)) return null;

  const toggle = async () => {
    const next = !open;
    setOpen(next);
    if (!next) return;
    try {
      setFailed(await loadFailed());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="queue">
      <button
        type="button"
        className={summary.failed > 0 ? "queue-btn alert" : "queue-btn"}
        aria-expanded={open}
        onClick={() => void toggle()}
      >
        {summary.pending} pending · {summary.failed} failed
      </button>
      {open && (
        <div className="queue-pop" role="region" aria-label="Failed reports">
          {error && <div className="muted">Could not load failed reports ({error}).</div>}
          {failed && failed.length === 0 && <div className="muted">No failed reports.</div>}
          {failed && failed.length > 0 && (
            <>
              <ul>
                {failed.map((r) => (
                  <li key={r.id}>
                    <div className="queue-item-head">
                      #{r.id} · {r.repo} / {r.project} · {r.commit_sha.slice(0, 10)}
                    </div>
                    <div className="queue-item-error">{r.error}</div>
                  </li>
                ))}
              </ul>
              <div className="muted queue-hint">
                Retry one with <code>POST /api/reports/&lt;id&gt;/retry</code> (X-API-Key).
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
