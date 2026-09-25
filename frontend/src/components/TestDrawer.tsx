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
  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} aria-hidden />
      <aside className="drawer" role="dialog" aria-modal="true" aria-label="Test detail">
        <div className="drawer-head">
          <div className="breadcrumb">
            {loaded ? breakable(`${history.test.repo} / ${history.test.project}`) : "Loading…"}
          </div>
          <button type="button" className="drawer-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        {loaded && (
          <>
            <h2 className="detail-title">{breakable(history.test.name)}</h2>
            <div className="detail-sub">
              {breakable(history.test.classname || history.test.suite)}
              {history.test.github_issue_number != null && (
                <> · issue #{history.test.github_issue_number}</>
              )}
            </div>
            <div className="detail-location">
              {history.location == null ? (
                <span className="muted">Location not reported by the test runner</span>
              ) : history.location.url ? (
                <a href={history.location.url} target="_blank" rel="noreferrer">
                  {breakable(history.location.path)}
                  {history.location.line != null && `:${history.location.line}`}
                </a>
              ) : (
                <span>
                  {breakable(history.location.path)}
                  {history.location.line != null && `:${history.location.line}`}
                </span>
              )}
            </div>
            <button
              type="button"
              className="quarantine-btn"
              onClick={() => onToggleQuarantine(history.test)}
            >
              {history.test.quarantined ? "Un-quarantine" : "Quarantine"}
            </button>
            <TestDetail history={history} />
          </>
        )}
      </aside>
    </>
  );
}
