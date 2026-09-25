import { useState } from "react";
import type { Execution, History } from "../api";
import { MarkShape, StatusMark, statusColor } from "./StatusMark";

const CELL = 18; // horizontal step per execution
const R = 5; // mark radius
const H = 46; // strip height

function fmtWhen(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  });
}

/** Execution history strip: oldest -> newest, left -> right.
 *  Hover any mark for commit/branch/time/message. */
function ExecutionStrip({ executions }: { executions: Execution[] }) {
  const [hover, setHover] = useState<{ x: number; e: Execution } | null>(null);
  const ordered = [...executions].reverse(); // API returns newest first
  const width = Math.max(ordered.length * CELL + CELL, 200);

  return (
    <div className="strip-wrap">
      <svg
        width="100%"
        viewBox={`0 0 ${width} ${H}`}
        preserveAspectRatio="xMinYMid meet"
        role="img"
        aria-label={`Execution history, oldest to newest: ${ordered
          .map((e) => e.status)
          .join(", ")}`}
        onMouseLeave={() => setHover(null)}
        style={{ display: "block" }}
      >
        <line x1={0} y1={H - 8} x2={width} y2={H - 8} stroke="var(--baseline)" strokeWidth={1} />
        {ordered.map((e, i) => {
          const cx = CELL / 2 + i * CELL;
          return (
            <g key={e.id}>
              {/* hit target larger than the mark */}
              <rect
                x={cx - CELL / 2} y={0} width={CELL} height={H}
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
          className="tooltip"
          style={{
            left: `min(${(hover.x / width) * 100}%, calc(100% - 200px))`,
            top: 0,
            transform: "translateY(-100%)",
          }}
        >
          <div className="t-status">
            <StatusMark status={hover.e.status} size={9} /> {hover.e.status}
            {hover.e.duration > 0 && ` · ${hover.e.duration.toFixed(2)}s`}
          </div>
          <div className="t-meta">
            {hover.e.commit_sha.slice(0, 10)} on {hover.e.branch} · {fmtWhen(hover.e.created_at)}
          </div>
          {hover.e.message && <div className="t-meta">{hover.e.message.slice(0, 140)}</div>}
        </div>
      )}
    </div>
  );
}

export function TestDetail({ history }: { history: History | null }) {
  if (!history) {
    return <div className="empty">Select a test to inspect its execution history.</div>;
  }
  const { test, executions } = history;
  const fails = executions.filter((e) => e.status === "failed" || e.status === "error").length;
  return (
    <div>
      <div className="detail-title">{test.name}</div>
      <div className="detail-sub">
        {test.classname || test.suite}
        {test.github_issue_number != null && <> · issue #{test.github_issue_number}</>}
      </div>

      <div className="facts">
        <div className="fact">
          <div className="label">Flakiness score</div>
          <div className="value">{test.flakiness_score.toFixed(2)}</div>
        </div>
        <div className="fact">
          <div className="label">Proven flakes</div>
          <div className="value">{test.confirmed_flake_count}</div>
        </div>
        <div className="fact">
          <div className="label">Failures (window)</div>
          <div className="value">
            {fails}/{executions.length}
          </div>
        </div>
      </div>

      <ExecutionStrip executions={executions} />
      <div className="strip-legend" aria-hidden>
        <span className="item"><StatusMark status="passed" /> passed</span>
        <span className="item"><StatusMark status="failed" /> failed</span>
        <span className="item"><StatusMark status="error" /> error</span>
        <span className="item"><StatusMark status="skipped" /> skipped</span>
      </div>

      <table className="exec-table">
        <thead>
          <tr>
            <th>Status</th>
            <th>Commit</th>
            <th>Branch</th>
            <th>When</th>
            <th>Message</th>
          </tr>
        </thead>
        <tbody>
          {executions.slice(0, 15).map((e) => (
            <tr key={e.id}>
              <td>
                <span className="chip">
                  <StatusMark status={e.status} size={9} /> {e.status}
                </span>
              </td>
              <td>{e.commit_sha.slice(0, 10)}</td>
              <td>{e.branch}</td>
              <td>{fmtWhen(e.created_at)}</td>
              <td className="msg" title={e.message}>{e.message || "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
