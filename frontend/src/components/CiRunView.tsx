import { useEffect, useRef, useState } from "react";
import {
  ApiError,
  fetchCiRunFailures,
  type CiRunFailedJob,
  type CiRunFailures,
  type CiRunTestFailure,
  type Tier,
} from "../api";
import { breakable } from "../breakable";
import { formatScore } from "../format";
import { IssueLink } from "./IssueLink";
import { scoreColor, TIER_CLASS } from "./scoreStyle";
import { StatusMark } from "./StatusMark";
import { SummaryBadge } from "./SummaryBadge";

/** While reports are still landing, ask again this often (issue #26). */
const POLL_MS = 5000;

/** True when FlakeRadar has no reports for the requested CI run. */
function isNotFound(e: unknown): boolean {
  return e instanceof ApiError && e.status === 404;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Score bar + tier, the leaderboard's Flakiness cell. */
function ScoreCell({ score, tier }: { score: number; tier: Tier }) {
  return (
    <div>
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
      </div>
      <span className={`mt-1 inline-block text-xs ${TIER_CLASS[tier]}`}>{tier}</span>
    </div>
  );
}

/** The failing Tests of one CI run attempt. */
function FailuresTable({
  failures,
  total,
  truncated,
  onOpenTest,
}: {
  failures: CiRunTestFailure[];
  total: number;
  truncated: boolean;
  onOpenTest: (testId: number) => void;
}) {
  if (failures.length === 0) {
    return <div className="px-4 py-10 text-center text-text-2">No test failures yet.</div>;
  }
  const th = "px-4 py-2 text-left text-xs font-medium text-muted";
  return (
    <div className="overflow-x-auto">
      <table className="leaderboard w-full border-collapse">
        <thead>
          <tr className="border-b border-line">
            <th className={th}>Test</th>
            <th className={th}>Project</th>
            <th className={`col-status ${th}`}>Status</th>
            <th className={th}>Flakiness</th>
            <th className={`col-proof ${th}`}>Proof</th>
            <th className={th}>Clean streak</th>
            <th className={`col-actions ${th} text-right`}>Issue</th>
          </tr>
        </thead>
        <tbody>
          {failures.map((f) => {
            // A Test that failed and then passed in the same Run is a retry,
            // which is the flakiness signal the developer is looking for.
            const retried = f.status === "passed" && f.failed_attempts > 0;
            return (
              <tr key={f.test_id} className="border-b border-line last:border-b-0">
                <td className="px-4 py-2.5 align-top">
                  <div className="font-semibold [overflow-wrap:anywhere]">
                    {f.quarantined && (
                      <span
                        className="mr-1.5 rounded border border-muted px-1 text-[11px] font-normal text-muted"
                        title="Quarantined — runner may skip"
                      >
                        ⏻ quarantined
                      </span>
                    )}
                    <button
                      type="button"
                      className="cursor-pointer text-left text-link hover:underline"
                      onClick={() => onOpenTest(f.test_id)}
                      title="Open this Test"
                    >
                      {breakable(f.name)}
                    </button>
                  </div>
                  <div className="text-xs text-muted [overflow-wrap:anywhere]">
                    {f.classname || f.suite}
                  </div>
                </td>
                <td className="px-4 py-2.5 align-top text-xs whitespace-nowrap text-text-2">
                  {f.project}
                </td>
                <td className="col-status px-4 py-2.5 align-top">
                  <span
                    className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-text-2"
                    title={retried ? `Failed ${f.failed_attempts}× before passing` : undefined}
                  >
                    <StatusMark status={f.status} />
                    {retried ? "passed on retry" : f.status}
                  </span>
                  {f.failure_category && (
                    <span
                      className="ml-2 inline-block rounded border border-line px-1 text-[11px] text-text-2"
                      title="Likely cause, from failure messages"
                    >
                      likely: {f.failure_category}
                    </span>
                  )}
                </td>
                <td className="px-4 py-2.5 align-top">
                  <ScoreCell score={f.flakiness_score} tier={f.tier} />
                </td>
                <td className="col-proof px-4 py-2.5 align-top text-xs whitespace-nowrap">
                  {f.confirmed_flake_count > 0 ? (
                    <span className="text-signal" title="Failed and passed on the same commit">
                      ⚠ {f.confirmed_flake_count}× proven
                    </span>
                  ) : (
                    <span className="text-muted">—</span>
                  )}
                </td>
                <td className="px-4 py-2.5 align-top text-xs tabular-nums text-text-2">
                  {f.clean_streak}
                </td>
                <td className="col-actions px-4 py-2.5 text-right align-top">
                  <IssueLink
                    number={f.github_issue_number}
                    url={f.github_issue_url}
                    className="rounded-full border border-line px-2 text-xs text-link hover:underline"
                    title="GitHub issue filed by FlakeRadar"
                  >
                    #{f.github_issue_number}
                  </IssueLink>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {truncated && (
        <div className="border-t border-line px-4 py-2 text-xs text-muted">
          {`Showing ${failures.length} of ${total}`}
          <span> failing tests — the endpoint caps the list.</span>
        </div>
      )}
    </div>
  );
}

/** The failed Jobs of one CI run attempt. */
function FailedJobsTable({
  jobs,
  onOpenJob,
}: {
  jobs: CiRunFailedJob[];
  onOpenJob: (jobId: number) => void;
}) {
  if (jobs.length === 0) {
    return <div className="px-4 py-10 text-center text-text-2">No failed jobs.</div>;
  }
  const th = "px-4 py-2 text-left text-xs font-medium text-muted";
  return (
    <div className="overflow-x-auto">
      <table className="leaderboard w-full border-collapse">
        <thead>
          <tr className="border-b border-line">
            <th className={th}>Pipeline / Job</th>
            <th className={`col-status ${th}`}>Outcome</th>
            <th className={th}>Flakiness</th>
            <th className={`col-actions ${th} text-right`}>CI job</th>
          </tr>
        </thead>
        <tbody>
          {jobs.map((j) => (
            <tr key={j.job_id} className="border-b border-line last:border-b-0">
              <td className="px-4 py-2.5 align-top">
                <div className="text-xs text-muted [overflow-wrap:anywhere]">
                  {breakable(j.pipeline)}
                </div>
                <button
                  type="button"
                  className="cursor-pointer text-left font-semibold text-link hover:underline [overflow-wrap:anywhere]"
                  onClick={() => onOpenJob(j.job_id)}
                  title="Open this Job"
                >
                  {breakable(j.name)}
                </button>
                {j.summary && <SummaryBadge className="ml-1.5" />}
              </td>
              <td className="col-status px-4 py-2.5 align-top">
                <span className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-text-2">
                  <StatusMark status="failed" size={9} />
                  {j.outcome}
                </span>
              </td>
              <td className="px-4 py-2.5 align-top">
                <ScoreCell score={j.flakiness_score} tier={j.tier} />
              </td>
              <td className="col-actions px-4 py-2.5 text-right align-top">
                <a
                  className="text-xs text-link underline-offset-2 hover:underline"
                  href={j.url}
                  target="_blank"
                  rel="noreferrer"
                  title="This Job execution in the CI provider"
                >
                  CI job
                </a>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The endpoint answers one attempt at a time, so the picker is a number input. */
function AttemptPicker({
  attempt,
  loadedAttempt,
  onAttempt,
}: {
  attempt: number | null;
  loadedAttempt: number | null;
  onAttempt: (attempt: number | null) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-xs text-text-2">
      Attempt
      <input
        type="number"
        min={1}
        className="control w-20 py-0.5 text-xs"
        value={attempt ?? loadedAttempt ?? ""}
        onChange={(e) => onAttempt(e.target.value === "" ? null : Number(e.target.value))}
      />
    </label>
  );
}

/**
 * One CI run attempt: its report counts, failing Tests and failed Jobs
 * (issue #26), read from `GET /api/ci-runs/{id}/failures`.
 */
export function CiRunView({
  repo,
  ciRun,
  attempt,
  onAttempt,
  onOpenTest,
  onOpenJob,
}: {
  repo: string;
  ciRun: string;
  attempt: number | null;
  onAttempt: (attempt: number | null) => void;
  onOpenTest: (testId: number) => void;
  onOpenJob: (jobId: number) => void;
}) {
  const [data, setData] = useState<CiRunFailures | null>(null);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  // Bumped when a request settles, so the next poll is timed from the answer
  // and a slow or failing one never stacks.
  const [settled, setSettled] = useState(0);

  // Only the newest request may write state: an answer for an attempt the
  // user has since picked again must not overwrite the view.
  const latest = useRef(0);

  useEffect(() => {
    const request = ++latest.current;
    fetchCiRunFailures(repo, ciRun, attempt)
      .then((d) => {
        if (request !== latest.current) return;
        setData(d);
        setMissing(false);
        setError(null);
        setSettled((n) => n + 1);
      })
      .catch((e) => {
        if (request !== latest.current) return;
        const notFound = isNotFound(e);
        // A 404 means nothing is known about this CI run, so drop whatever is
        // on screen rather than showing a different attempt's rows.
        if (notFound) setData(null);
        setMissing(notFound);
        setError(e instanceof Error ? e.message : String(e));
        setSettled((n) => n + 1);
      });
  }, [repo, ciRun, attempt, reload]);

  // Reports still landing: ask again until the attempt is fully processed.
  useEffect(() => {
    if (!data || data.reports.pending <= 0) return;
    const timer = setTimeout(() => setReload((n) => n + 1), POLL_MS);
    return () => clearTimeout(timer);
  }, [data, settled]);

  const header = (title: string) => (
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
      <h2 className="text-[15px] font-semibold">{title}</h2>
      <AttemptPicker
        attempt={attempt}
        loadedAttempt={data?.ci_run_attempt ?? null}
        onAttempt={onAttempt}
      />
    </div>
  );

  if (missing) {
    return (
      <section className="rounded-xl border border-line bg-surface">
        {header(`CI run ${ciRun}`)}
        <div className="px-4 py-10 text-center text-text-2">
          FlakeRadar has no reports for this CI run
          {attempt != null && <span className="text-muted"> (attempt {attempt}).</span>}
        </div>
      </section>
    );
  }

  if (!data) {
    return (
      <section className="rounded-xl border border-line bg-surface">
        {header(`CI run ${ciRun}`)}
        <div className="px-4 py-10 text-center text-text-2">
          {error ? `Could not read CI run ${ciRun}: ${error}` : "Loading…"}
        </div>
      </section>
    );
  }

  const { reports } = data;
  return (
    <div className="flex flex-col gap-4">
      <section className="rounded-xl border border-line bg-surface">
        {header(`CI run ${data.ci_run_id}`)}
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-line px-4 py-2 text-xs">
          <span className="text-text-2">{plural(reports.processed, "report")} processed</span>
          {reports.pending > 0 && (
            <span role="status" className="text-signal">
              {`Still processing ${reports.pending} reports`}
            </span>
          )}
          {reports.failed > 0 && (
            <span role="alert" className="text-critical">
              {`${plural(reports.failed, "report")} failed to process`}
            </span>
          )}
          {error && <span className="text-critical">Last refresh failed: {error}</span>}
        </div>
        <FailuresTable
          failures={data.failures}
          total={data.total_failures}
          truncated={data.truncated}
          onOpenTest={onOpenTest}
        />
      </section>

      <section className="rounded-xl border border-line bg-surface">
        <div className="border-b border-line px-4 py-3">
          <h2 className="text-[15px] font-semibold">Failed jobs</h2>
        </div>
        <FailedJobsTable jobs={data.failed_jobs} onOpenJob={onOpenJob} />
      </section>
    </div>
  );
}
