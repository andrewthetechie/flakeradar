// Shared result-table cells: the Flakiness score bar, the Proven-flake cell, the
// status + issue cell and the pills. Copied from Leaderboard.tsx so every scored
// surface (leaderboards, CI run view) renders identical markup from one module.
import type { MouseEvent } from "react";
import type { FailureCategory, Tier, Trend } from "../api";
import { formatScore } from "../format";
import { IssueLink } from "./IssueLink";
import { scoreColor, TIER_CLASS } from "./scoreStyle";
import { StatusMark } from "./StatusMark";
import { TrendMark } from "./TrendMark";

/** Shown next to a test name while the runner may skip it (ADR 0004: Tests only). */
export function QuarantinedPill() {
  return (
    <span
      className="mr-1.5 rounded border border-muted px-1 text-[11px] font-normal text-muted"
      title="Quarantined — runner may skip"
    >
      ⏻ quarantined
    </span>
  );
}

/** Shown next to a test name when most tests in its run failed together (ADR 0013). */
export function InfraSuspectPill() {
  return (
    <span
      className="mr-1.5 rounded border border-muted px-1 text-[11px] font-normal text-muted"
      title="Most tests in this run failed together, so it is likely infrastructure. Scoring leaves this run out."
    >
      ⚠ infrastructure?
    </span>
  );
}

/** Shown next to a test name when it is failing on the Default branch (issue #29). */
export function BrokenPill() {
  return (
    <span
      className="mr-1.5 rounded border border-critical px-1 text-[11px] font-normal text-critical"
      title="Broken — failing on the default branch"
    >
      ✕ broken on default branch
    </span>
  );
}

/** Score bar, formatted score, trend mark, tier label and optional likely-cause badge. */
export function FlakinessCell({
  score,
  tier,
  trend,
  category,
}: {
  score: number;
  tier: Tier;
  trend?: Trend | null;
  category?: FailureCategory | null;
}) {
  return (
    <>
      <div className="flex min-w-18 items-center gap-2 sm:min-w-32">
        <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-line">
          <div
            className="h-full rounded-full"
            style={{
              width: `${Math.max(score * 100, score > 0 ? 4 : 0)}%`,
              background: scoreColor(score),
            }}
          />
        </div>
        <span className="w-9 text-right tabular-nums text-text-2">{formatScore(score)}</span>
        <TrendMark trend={trend ?? null} />
      </div>
      <span className={`mt-1 inline-block text-xs ${TIER_CLASS[tier]}`}>{tier}</span>
      {category && (
        <span
          className="mt-1 ml-2 inline-block rounded border border-line px-1 text-[11px] text-text-2"
          title="Likely cause, from failure messages"
        >
          likely: {category}
        </span>
      )}
    </>
  );
}

/** Proven-flake cell: failures confirmed by a pass on the same commit. */
export function ProvenCell({ count }: { count: number }) {
  return (
    <td className="col-proof px-4 py-2.5 align-top text-xs whitespace-nowrap">
      {count > 0 ? (
        <span className="text-signal" title="Failed and passed on the same commit">
          ⚠ {count}× proven
        </span>
      ) : (
        <span className="text-muted">—</span>
      )}
    </td>
  );
}

/** The `#N` pill linking FlakeRadar's issue, or nothing when no issue exists. */
export function IssuePill({
  number,
  url,
  closed,
  className,
  onClick,
}: {
  number: number | null;
  url: string | null;
  /** The linked issue was closed as not planned (CONTEXT.md). */
  closed?: boolean;
  className?: string;
  onClick?: (e: MouseEvent) => void;
}) {
  return (
    <IssueLink
      number={number}
      url={url}
      closed={closed}
      onClick={onClick}
      className={`rounded-full border border-line px-2 ${className ? `${className} ` : ""}text-link hover:underline`}
      title="GitHub issue filed by FlakeRadar"
    >
      #{number}
    </IssueLink>
  );
}

/** Last status + issue pill, the two leaderboards' shared cell. */
export function StatusIssueCell({
  status,
  issueNumber,
  issueUrl,
  issueClosed,
}: {
  status: string;
  issueNumber: number | null;
  issueUrl: string | null;
  /** The linked issue was closed as not planned (CONTEXT.md). */
  issueClosed?: boolean;
}) {
  return (
    <td className="col-status px-4 py-2.5 align-top">
      <span className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-text-2">
        <StatusMark status={status} /> {status}
        <IssuePill
          number={issueNumber}
          url={issueUrl}
          closed={issueClosed}
          onClick={(e) => e.stopPropagation()}
        />
      </span>
    </td>
  );
}
