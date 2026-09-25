import { useState } from "react";
import type { Execution, History } from "../api";
import { formatScore } from "../format";
import { MarkShape, StatusMark, statusColor } from "./StatusMark";

const CELL = 18; // horizontal step per execution
const R = 5; // mark radius
const H = 46; // strip height

function fmtWhen(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Execution history strip: oldest -> newest, left -> right.
 *  Hover any mark for commit/branch/time/message. */
function ExecutionStrip({ executions }: { executions: Execution[] }) {
  const [hover, setHover] = useState<{ x: number; e: Execution } | null>(null);
  const ordered = [...executions].reverse(); // API returns newest first
  const width = Math.max(ordered.length * CELL + CELL, 200);

  return (
    <div className="relative">
      <svg
        width="100%"
        viewBox={`0 0 ${width} ${H}`}
        preserveAspectRatio="xMinYMid meet"
        role="img"
        aria-label={`Execution history, oldest to newest: ${ordered
          .map((e) => e.status)
          .join(", ")}`}
        onMouseLeave={() => setHover(null)}
        style={{ display: "block", maxWidth: width }} // shrink for long histories, never enlarge
      >
        <line x1={0} y1={H - 8} x2={width} y2={H - 8} stroke="var(--baseline)" strokeWidth={1} />
        {ordered.map((e, i) => {
          const cx = CELL / 2 + i * CELL;
          return (
            <g key={e.id}>
              {/* hit target larger than the mark */}
              <rect
                x={cx - CELL / 2}
                y={0}
                width={CELL}
                height={H}
                fill="transparent"
                onMouseEnter={() => setHover({ x: cx, e })}
              />
              <MarkShape
                status={e.status}
                cx={cx}
                cy={H / 2 - 4}
                r={hover?.e.id === e.id ? R + 1.5 : R}
                color={statusColor(e.status)}
              />
            </g>
          );
        })}
      </svg>
      {hover && (
        <div
          className="pointer-events-none absolute z-10 max-w-65 rounded-lg border border-line bg-surface-2 px-2.5 py-2 text-xs shadow-xl"
          style={{
            left: `min(${(hover.x / width) * 100}%, calc(100% - 200px))`,
            top: 0,
            transform: "translateY(-100%)",
          }}
        >
          <div className="flex items-center gap-1.5 font-semibold">
            <StatusMark status={hover.e.status} size={9} /> {hover.e.status}
            {hover.e.duration > 0 && ` · ${hover.e.duration.toFixed(2)}s`}
          </div>
          <div className="text-text-2">
            <span className="font-mono">{hover.e.commit_sha.slice(0, 10)}</span> on {hover.e.branch}{" "}
            · {fmtWhen(hover.e.created_at)}
          </div>
          {hover.e.message && <div className="text-text-2">{hover.e.message.slice(0, 140)}</div>}
        </div>
      )}
    </div>
  );
}

export function TestDetail({ history }: { history: History }) {
  const { executions } = history;
  const fails = executions.filter((e) => e.status === "failed" || e.status === "error").length;
  const latestFailure = executions.find((e) => e.status === "failed" || e.status === "error");
  return (
    <div>
      <div className="mb-4 flex flex-wrap gap-x-8 gap-y-2">
        <div>
          <div className="text-xs text-muted">Flakiness score</div>
          <div className="text-lg font-semibold tabular-nums">
            {formatScore(history.test.flakiness_score)}
          </div>
        </div>
        <div>
          <div className="text-xs text-muted">Proven flakes</div>
          <div
            className={`text-lg font-semibold tabular-nums ${history.test.confirmed_flake_count > 0 ? "text-signal" : ""}`}
          >
            {history.test.confirmed_flake_count}
          </div>
        </div>
        <div>
          <div className="text-xs text-muted">Failures (window)</div>
          <div className="text-lg font-semibold tabular-nums">
            {fails}/{executions.length}
          </div>
        </div>
      </div>

      {latestFailure && (latestFailure.details || latestFailure.message) && (
        <details className="group mb-5" open>
          <summary className="cursor-pointer text-xs text-text-2 hover:text-text">
            Latest failure ·{" "}
            <span className="font-mono">{latestFailure.commit_sha.slice(0, 10)}</span> on{" "}
            {latestFailure.branch}
          </summary>
          <pre className="mt-2 max-h-80 overflow-auto rounded-lg border border-line bg-page p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] text-text">
            {latestFailure.details || latestFailure.message}
          </pre>
        </details>
      )}

      <ExecutionStrip executions={executions} />
      <div className="mt-2 mb-1 flex flex-wrap gap-4 text-xs text-text-2" aria-hidden>
        <span className="inline-flex items-center gap-1.5">
          <StatusMark status="passed" /> passed
        </span>
        <span className="inline-flex items-center gap-1.5">
          <StatusMark status="failed" /> failed
        </span>
        <span className="inline-flex items-center gap-1.5">
          <StatusMark status="error" /> error
        </span>
        <span className="inline-flex items-center gap-1.5">
          <StatusMark status="skipped" /> skipped
        </span>
      </div>

      <table className="mt-4 w-full border-collapse text-xs">
        <thead>
          <tr className="border-b border-line text-left text-muted">
            <th className="py-1.5 pr-3 font-medium">Status</th>
            <th className="py-1.5 pr-3 font-medium">Commit</th>
            <th className="py-1.5 pr-3 font-medium">Branch</th>
            <th className="py-1.5 pr-3 font-medium">When</th>
            <th className="py-1.5 font-medium">Message</th>
          </tr>
        </thead>
        <tbody>
          {executions.slice(0, 15).map((e) => (
            <tr
              key={e.id}
              className="border-b border-line tabular-nums whitespace-nowrap [&>td]:py-1.5 [&>td]:pr-3"
            >
              <td>
                <span className="inline-flex items-center gap-1.5 text-text-2">
                  <StatusMark status={e.status} size={9} /> {e.status}
                </span>
              </td>
              <td className="font-mono">{e.commit_sha.slice(0, 10)}</td>
              <td>{e.branch}</td>
              <td>{fmtWhen(e.created_at)}</td>
              <td className="min-w-40 whitespace-normal text-text-2 [overflow-wrap:anywhere]">
                {e.message || "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
