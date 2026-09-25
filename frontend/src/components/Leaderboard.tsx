import type { Scope, TestCase } from "../api";
import { breakable } from "../breakable";
import { formatScore } from "../format";
import { IssueLink } from "./IssueLink";
import { StatusMark } from "./StatusMark";

// Sequential blue: stronger = worse (the scale flips per theme in styles.css).
function scoreColor(score: number): string {
  if (score >= 0.75) return "var(--seq-650)";
  if (score >= 0.5) return "var(--seq-550)";
  if (score >= 0.25) return "var(--seq-400)";
  return "var(--seq-250)";
}

const TIER_CLASS: Record<TestCase["tier"], string> = {
  flaky: "font-semibold text-signal",
  suspect: "text-text-2",
  stable: "text-muted",
};

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
  selectedId,
  onSelect,
  onToggleQuarantine,
}: {
  tests: TestCase[];
  scope: Scope;
  showStable: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onToggleQuarantine: (t: TestCase) => void;
}) {
  if (tests.length === 0) {
    return (
      <div className="px-4 py-10 text-center text-text-2">
        {showStable ? (
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
                  {t.quarantined && (
                    <span
                      className="mr-1.5 rounded border border-muted px-1 text-[11px] font-normal text-muted"
                      title="Quarantined — runner may skip"
                    >
                      ⏻ quarantined
                    </span>
                  )}
                  {breakable(t.name)}
                </div>
                <div className="text-xs text-muted [overflow-wrap:anywhere]">
                  {t.classname || t.suite}
                </div>
              </td>
              <td className="px-4 py-2.5 align-top">
                <div className="flex min-w-18 items-center gap-2 sm:min-w-32">
                  <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-line">
                    <div
                      className="h-full rounded-full"
                      style={{
                        width: `${Math.max(t.flakiness_score * 100, t.flakiness_score > 0 ? 4 : 0)}%`,
                        background: scoreColor(t.flakiness_score),
                      }}
                    />
                  </div>
                  <span className="w-9 text-right tabular-nums text-text-2">
                    {formatScore(t.flakiness_score)}
                  </span>
                </div>
                <span className={`mt-1 inline-block text-xs ${TIER_CLASS[t.tier]}`}>{t.tier}</span>
              </td>
              <td className="col-proof px-4 py-2.5 align-top text-xs whitespace-nowrap">
                {t.confirmed_flake_count > 0 ? (
                  <span className="text-signal" title="Failed and passed on the same commit">
                    ⚠ {t.confirmed_flake_count}× proven
                  </span>
                ) : (
                  <span className="text-muted">—</span>
                )}
              </td>
              <td className="col-status px-4 py-2.5 align-top">
                <span className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-text-2">
                  <StatusMark status={t.last_status} /> {t.last_status}
                  <IssueLink
                    number={t.github_issue_number}
                    url={t.github_issue_url}
                    onClick={(e) => e.stopPropagation()}
                    className="rounded-full border border-line px-2 text-link hover:underline"
                    title="GitHub issue filed by FlakeRadar"
                  >
                    #{t.github_issue_number}
                  </IssueLink>
                </span>
              </td>
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
