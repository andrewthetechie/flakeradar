import { useEffect } from "react";
import type { History, TestCase } from "../api";
import { breakable } from "../breakable";
import { TestDetail } from "./TestDetail";

/** Slide-over panel for one Test. The leaderboard keeps the full width. */
export function TestDrawer({
  testId, history, onClose, onToggleQuarantine,
}: {
  testId: number;
  history: History | null;
  onClose: () => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const loaded = history != null && history.test.id === testId;
  const loc = history?.location;
  return (
    <>
      <div className="fixed inset-0 z-20 bg-scrim" onClick={onClose} aria-hidden />
      <aside
        className="fixed inset-y-0 right-0 z-21 w-full max-w-190 animate-slide-in overflow-y-auto border-l border-line bg-surface px-5 pt-4 pb-8 shadow-2xl"
        role="dialog"
        aria-modal="true"
        aria-label="Test detail"
      >
        <div className="mb-2 flex items-start justify-between gap-3">
          <div className="text-xs text-link [overflow-wrap:anywhere]">
            {loaded ? breakable(`${history.test.repo} / ${history.test.project}`) : "Loading…"}
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
        {loaded && (
          <>
            <h2 className="text-lg font-semibold leading-snug [overflow-wrap:anywhere]">
              {breakable(history.test.name)}
            </h2>
            <div className="mb-3 text-xs text-muted [overflow-wrap:anywhere]">
              {breakable(history.test.classname || history.test.suite)}
              {history.test.github_issue_number != null && (
                <> · issue #{history.test.github_issue_number}</>
              )}
            </div>
            <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0 font-mono text-xs [overflow-wrap:anywhere]">
                {loc == null ? (
                  <span className="font-sans text-muted">Location not reported by the test runner</span>
                ) : loc.url ? (
                  <a className="text-link underline-offset-2 hover:underline" href={loc.url}
                    target="_blank" rel="noreferrer">
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
              <button
                type="button"
                className="btn"
                onClick={() => onToggleQuarantine(history.test)}
              >
                {history.test.quarantined ? "Un-quarantine" : "Quarantine"}
              </button>
            </div>
            <TestDetail history={history} />
          </>
        )}
      </aside>
    </>
  );
}
