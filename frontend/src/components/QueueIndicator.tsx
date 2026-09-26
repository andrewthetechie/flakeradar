import { useState } from "react";
import type { ReportInfo, ReportSummary } from "../api";

/** "3 pending · 1 failed" — the only place a broken CI upload becomes visible.
 *  Hidden while the queue is empty and nothing has failed. */
export function QueueIndicator({
  summary,
  loadFailed,
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
    <div className="relative">
      <button
        type="button"
        className={`rounded-full border px-3 py-1 text-xs tabular-nums ${
          summary.failed > 0
            ? "border-critical/70 bg-critical/10 text-critical"
            : "border-line text-text-2 hover:text-text"
        }`}
        aria-expanded={open}
        onClick={() => void toggle()}
      >
        {summary.pending} pending · {summary.failed} failed
      </button>
      {open && (
        <div
          className="absolute top-[calc(100%+6px)] left-0 z-15 max-h-90 w-[min(420px,calc(100vw-32px))] overflow-y-auto rounded-lg border border-line bg-surface-2 px-3 py-2.5 text-xs shadow-xl sm:right-0 sm:left-auto"
          role="region"
          aria-label="Failed reports"
        >
          {error && <div className="text-muted">Could not load failed reports ({error}).</div>}
          {failed && failed.length === 0 && <div className="text-muted">No failed reports.</div>}
          {failed && failed.length > 0 && (
            <>
              <ul>
                {failed.map((r) => (
                  <li key={r.id} className="border-b border-line py-1.5 last:border-b-0">
                    <div className="font-semibold [overflow-wrap:anywhere]">
                      #{r.id} ·{" "}
                      {r.project == null ? `${r.repo} · pipeline` : `${r.repo} / ${r.project}`} ·{" "}
                      {r.commit_sha.slice(0, 10)}
                    </div>{" "}
                    <div className="text-critical [overflow-wrap:anywhere]">{r.error}</div>
                  </li>
                ))}
              </ul>
              <div className="mt-2 text-muted">
                Retry one with{" "}
                <code className="font-mono text-text-2">POST /api/reports/&lt;id&gt;/retry</code>{" "}
                (X-API-Key).
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
