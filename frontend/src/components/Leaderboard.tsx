import type { Scope, TestCase } from "../api";
import { breakable } from "../breakable";
import { FlakinessCell, ProvenCell, BrokenPill, QuarantinedPill, StatusIssueCell } from "./resultCells";

/** Where a row lives, shown only when the view spans more than that. */
export function scopeLabel(t: TestCase, scope: Scope): string | null {
  if (!scope.repo) return `${t.repo} · ${t.project}`;
  if (!scope.project) return t.project;
  return null;
}

export function Leaderboard({
  tests,
  scope,
  showStable,
  search = "",
  selectedId,
  onSelect,
  onToggleQuarantine,
}: {
  tests: TestCase[];
  scope: Scope;
  showStable: boolean;
  search?: string;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  if (tests.length === 0) {
    return (
      <div className="px-4 py-10 text-center text-text-2">
        {search ? (
          <>No tests match.</>
        ) : showStable ? (
          <>
            No test data yet. POST a JUnit XML report to{" "}
            <code className="font-mono text-text">/api/ingest</code> — see the README for the
            one-line CI snippet.
          </>
        ) : (
          <>No flaky or suspect tests in this view. Tick “Show stable tests” to list every test.</>
        )}
      </div>
    );
  }
  const th = "px-4 py-2 text-left text-xs font-medium text-muted";
  return (
    <table className="leaderboard w-full border-collapse">
      <thead>
        <tr className="border-b border-line">
          <th className={th}>Test</th>
          <th className={th}>Flakiness</th>
          <th className={`col-proof ${th}`}>Proof</th>
          <th className={`col-status ${th}`}>Last status</th>
          <th className={`col-actions ${th} text-right`}>Quarantine</th>
        </tr>
      </thead>
      <tbody>
        {tests.map((t) => {
          const label = scopeLabel(t, scope);
          const selected = t.id === selectedId;
          return (
            <tr
              key={t.id}
              className={`cursor-pointer border-b border-line last:border-b-0 hover:bg-surface-2 ${
                selected ? "bg-surface-2 shadow-[inset_3px_0_0_var(--signal)]" : ""
              }`}
              onClick={() => onSelect(t.id)}
            >
              <td className="px-4 py-2.5 align-top">
                {label && <div className="text-xs text-link [overflow-wrap:anywhere]">{label}</div>}
                <div className="font-semibold [overflow-wrap:anywhere]">
                  {t.broken && <BrokenPill />}
                  {t.quarantined && <QuarantinedPill />}
                  {breakable(t.name)}
                </div>
                <div className="text-xs text-muted [overflow-wrap:anywhere]">
                  {t.classname || t.suite}
                </div>
              </td>
              <td className="px-4 py-2.5 align-top">
                <FlakinessCell
                  score={t.flakiness_score}
                  tier={t.tier}
                  trend={t.trend}
                  category={t.failure_category}
                />
              </td>
              <ProvenCell count={t.confirmed_flake_count} />
              <StatusIssueCell
                status={t.last_status}
                issueNumber={t.github_issue_number}
                issueUrl={t.github_issue_url}
              />
              <td className="col-actions px-4 py-2.5 text-right align-top">
                <button
                  type="button"
                  className="btn whitespace-nowrap"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleQuarantine(t);
                  }}
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
