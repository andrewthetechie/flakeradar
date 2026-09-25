import type { Scope, TestCase } from "../api";
import { StatusMark } from "./StatusMark";

// Sequential blue: darker = worse, so severity reads as magnitude.
function scoreColor(score: number): string {
  if (score >= 0.75) return "var(--seq-650)";
  if (score >= 0.5) return "var(--seq-550)";
  if (score >= 0.25) return "var(--seq-400)";
  return "var(--seq-250)";
}

/** Where a row lives, shown only when the view spans more than that. */
export function scopeLabel(t: TestCase, scope: Scope): string | null {
  if (!scope.repo) return `${t.repo} · ${t.project}`;
  if (!scope.project) return t.project;
  return null;
}

export function Leaderboard({
  tests, scope, showStable, selectedId, onSelect, onToggleQuarantine,
}: {
  tests: TestCase[];
  scope: Scope;
  showStable: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  if (tests.length === 0) {
    return showStable ? (
      <div className="empty">
        No test data yet. POST a JUnit XML report to <code>/api/ingest</code> — see
        the README for the one-line CI snippet.
      </div>
    ) : (
      <div className="empty">
        No flaky or suspect tests in this view. Tick “Show stable tests” to list every test.
      </div>
    );
  }
  return (
    <table className="leaderboard">
      <thead>
        <tr>
          <th>Test</th>
          <th>Flakiness</th>
          <th>Proof</th>
          <th>Last status</th>
          <th>Quarantine</th>
        </tr>
      </thead>
      <tbody>
        {tests.map((t) => {
          const label = scopeLabel(t, scope);
          return (
            <tr
              key={t.id}
              className={t.id === selectedId ? "selected" : ""}
              onClick={() => onSelect(t.id)}
            >
              <td>
                {label && <div className="test-scope">{label}</div>}
                <div className="test-name">
                  {t.quarantined && (
                    <span className="badge-quarantined" title="Quarantined — runner may skip">
                      ⏻ quarantined
                    </span>
                  )}
                  {t.name}
                </div>
                <div className="test-class">{t.classname || t.suite}</div>
              </td>
              <td>
                <div className="meter">
                  <div className="track">
                    <div
                      className="fill"
                      style={{
                        width: `${Math.max(t.flakiness_score * 100, t.flakiness_score > 0 ? 4 : 0)}%`,
                        background: scoreColor(t.flakiness_score),
                      }}
                    />
                  </div>
                  <span className="num">{t.flakiness_score.toFixed(2)}</span>
                </div>
                <span className={`tier tier-${t.tier}`}>{t.tier}</span>
              </td>
              <td>
                {t.confirmed_flake_count > 0 ? (
                  <span className="chip" title="Failed and passed on the same commit">
                    ⚠ {t.confirmed_flake_count}× proven
                  </span>
                ) : (
                  <span className="chip">—</span>
                )}
              </td>
              <td>
                <span className="chip">
                  <StatusMark status={t.last_status} /> {t.last_status}
                  {t.github_issue_number != null && (
                    <span className="badge-issue">#{t.github_issue_number}</span>
                  )}
                </span>
              </td>
              <td>
                <button
                  className="quarantine-btn"
                  onClick={(e) => { e.stopPropagation(); onToggleQuarantine(t); }}
                >
                  {t.quarantined ? "Un-quarantine" : "Quarantine"}
                </button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
