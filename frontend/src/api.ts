// Typed contract with the FastAPI backend (mirrors backend/app/schemas.py).

// The list-filter table (urlState.ts) declares each filter's wire name and value
// spelling; fetchTests/fetchJobs below build their filter params from it. The
// module graph stays acyclic: urlState.ts imports only *types* from this module,
// so importing its table at runtime here cannot loop back.
import { LIST_FILTERS, type FailureCategory, type WireParam } from "./urlState";
// Home of the taxonomy is urlState.ts, whose `cause` descriptor reads it at
// table init. Re-exported so every importer keeps its `from "./api"` import.
export { FAILURE_CATEGORIES } from "./urlState";
export type { FailureCategory } from "./urlState";

export type Tier = "flaky" | "suspect" | "stable";
export type SortKey = "score" | "last_seen" | "proven" | "impact";
/** Issue link filter: Tests with a linked GitHub issue, or without one. */
export type IssueFilter = "linked" | "none";
export type Trend = "worsening" | "improving" | "steady";

export interface ScorePoint {
  day: string; // "YYYY-MM-DD", UTC
  flakiness_score: number;
  confirmed_flake_count: number;
  executions: number;
  failures: number;
}

export interface TestCase {
  id: number;
  repo: string;
  project: string;
  fingerprint: string;
  suite: string;
  classname: string;
  name: string;
  file: string | null;
  line: number | null;
  flakiness_score: number;
  tier: Tier;
  confirmed_flake_count: number;
  failure_category: FailureCategory | null;
  clean_streak: number;
  fail_streak: number;
  /** Test failing on the Default branch broken_streak runs in a row (CONTEXT.md). */
  broken: boolean;
  trend: Trend | null;
  last_status: string;
  last_seen_at: string;
  quarantined: boolean;
  quarantined_at: string | null;
  quarantine_reason: string | null;
  quarantine_expires_at: string | null;
  /** When the acknowledgement lapses (RFC 3339), or null when unacknowledged. */
  acknowledged_until: string | null;
  /** Why a person acknowledged the Test, or null. */
  acknowledge_reason: string | null;
  github_issue_number: number | null;
  github_issue_url: string | null;
  github_issue_closed: boolean;
  /** How many CI runs the Test failed in within the impact window. */
  impacted_runs: number;
  /** How many non-default branches the Test's failures broke within the impact window. */
  impacted_branches: number;
  /** How many seconds of CI time the Test's failures cost within the impact window. */
  time_lost_seconds: number;
}

export interface TestPage {
  items: TestCase[];
  total: number;
  page: number;
  page_size: number;
}

export interface ProjectInfo {
  name: string;
  root: string;
}

export interface RepoInfo {
  name: string;
  projects: ProjectInfo[];
}

export interface Execution {
  id: number;
  status: string;
  duration: number;
  message: string;
  details: string;
  created_at: string;
  commit_sha: string;
  branch: string;
  ci_run_id: string;
  attempt: number; // 0 = first try in its Run; >0 = a retry
  failure_category: FailureCategory | null;
}

export interface Location {
  path: string;
  line: number | null;
  url: string | null;
}

export interface History {
  test: TestCase;
  location: Location | null;
  last_failing_sha: string | null;
  last_failing_branch: string | null;
  executions: Execution[];
  jobs: TestJobLink[];
  score_history: ScorePoint[];
}

export interface TestJobLink {
  job_id: number;
  pipeline: string;
  name: string;
}

export interface Summary {
  total_tests: number;
  flaky_tests: number;
  suspect_tests: number;
  confirmed_flaky_tests: number;
  /** Tests that became flaky in the last 7 days. */
  new_flaky_7d: number;
  total_runs: number;
  /** Executions and failures summed from score history over the last 30 days. */
  executions_30d: number;
  failures_30d: number;
  flake_threshold: number;
  category_counts: Record<FailureCategory, number>;
}

/** Which Tests a view covers. `project` is only meaningful with `repo`. */
export interface Scope {
  repo: string | null;
  project: string | null;
}

export interface TestQuery extends Scope {
  page: number;
  pageSize: number;
  sort: SortKey;
  showStable: boolean;
  category: FailureCategory | null;
  quarantined: boolean;
  /** List only acknowledged Tests, sent as `acknowledged=true`. */
  acknowledged: boolean;
  /** List only Tests broken on the Default branch, or false for any. */
  broken: boolean;
  issue: IssueFilter | null;
  /** List only Tests that became flaky in the last N days, or null for any. */
  newWithinDays: number | null;
  q: string;
}

/** A failed HTTP response; `message` keeps the "<url> -> <status>" wording. */
export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function getJson<T>(url: string): Promise<T> {
  const resp = await fetch(url);
  if (!resp.ok) throw new ApiError(`${url} -> ${resp.status}`, resp.status);
  return resp.json() as Promise<T>;
}

/**
 * Newest-request-wins guard: whoever answers last is not necessarily whoever
 * should write state, so every request carries a token and only the newest one
 * may. Framework-free on purpose, so a component can hold one for its lifetime
 * or a plain caller can create one per effect; timers stay with the caller.
 */
export interface LatestWins {
  /** Begins a request, superseding every earlier one. */
  start: () => number;
  /** May the answer for this token still write state? */
  current: (token: number) => boolean;
  /** Supersede every outstanding answer (unmount, scope abandoned). */
  cancel: () => void;
}

export function latestWins(): LatestWins {
  let newest = 0;
  return {
    start: () => ++newest,
    current: (token: number) => token === newest,
    cancel: () => {
      newest++;
    },
  };
}

export function scopeParams(scope: Scope): URLSearchParams {
  const params = new URLSearchParams();
  if (scope.repo) {
    params.set("repo", scope.repo);
    if (scope.project) params.set("project", scope.project);
  }
  return params;
}

function withQuery(path: string, params: URLSearchParams): string {
  const q = params.toString();
  return q ? `${path}?${q}` : path;
}

export const fetchRepos = () => getJson<RepoInfo[]>("/api/repos");

export const fetchSummary = (scope: Scope) =>
  getJson<Summary>(withQuery("/api/summary", scopeParams(scope)));

/**
 * Sets the endpoint's filter params from the table, in the table's declaration
 * order. Nothing downstream reads the sequence — the backend deserialises the
 * whole query — so the parameter set is the contract.
 */
function setWireParams<Q extends TestQuery | JobQuery>(
  params: URLSearchParams,
  q: Q,
  wires: (WireParam<Q> | undefined)[],
): void {
  for (const w of wires) {
    if (!w) continue;
    const v = w.value(q);
    if (v != null) params.set(w.param, v);
  }
}

export function fetchTests(q: TestQuery): Promise<TestPage> {
  // Scope and paging stay hand-written: they are not list filters.
  const params = scopeParams(q);
  params.set("page", String(q.page));
  params.set("page_size", String(q.pageSize));
  // Every filter the Tests endpoint accepts comes from the table, in the
  // table's declaration order: q, sort, include_stable, category, quarantined,
  // broken, acknowledged, has_issue, new_within_days. The order carries no
  // meaning; the parameter set is the contract.
  setWireParams(
    params,
    q,
    LIST_FILTERS.map((d) => d.wire?.tests),
  );
  return getJson<TestPage>(withQuery("/api/tests", params));
}

export const fetchHistory = (id: number) => getJson<History>(`/api/tests/${id}/history?limit=60`);

// --- Jobs (task 06) ------------------------------------------------------

export interface Job {
  id: number;
  repo: string;
  provider: string;
  pipeline: string;
  name: string;
  flakiness_score: number;
  tier: Tier;
  confirmed_flake_count: number;
  clean_streak: number;
  trend: Trend | null;
  last_status: string;
  last_seen_at: string;
  summary: boolean; // a summary Job (CONTEXT.md) that runs no tests; its explained failures are never scored
  /** When the acknowledgement lapses (RFC 3339), or null when unacknowledged. */
  acknowledged_until: string | null;
  /** Why a person acknowledged the Job, or null. */
  acknowledge_reason: string | null;
  github_issue_number: number | null;
  github_issue_url: string | null;
  github_issue_closed: boolean;
  /** How many CI runs the Job failed in within the impact window. */
  impacted_runs: number;
  /** How many non-default branches the Job's failures broke within the impact window. */
  impacted_branches: number;
  /** How many seconds of CI time the Job's failures cost within the impact window. */
  time_lost_seconds: number;
}

export interface JobPage {
  items: Job[];
  total: number;
  page: number;
  page_size: number;
}

export interface JobSummary {
  total_jobs: number;
  flaky_jobs: number;
  suspect_jobs: number;
  confirmed_flaky_jobs: number;
  /** Jobs that became flaky in the last 7 days. */
  new_flaky_7d: number;
  /** Executions and failures summed from job score history over the last 30 days. */
  job_executions_30d: number;
  job_failures_30d: number;
  flake_threshold: number;
  /** Distinct Pipeline names of the scope, sorted; feeds the Jobs filter. */
  pipelines: string[];
}

export interface ExplainingTest {
  test_id: number;
  project: string;
  classname: string;
  name: string;
  status: string;
}

export interface ExplainingJob {
  job_id: number;
  pipeline: string;
  name: string;
  url: string;
}

export interface JobExecution {
  id: number;
  status: string; // passed | failed | skipped (as stored)
  outcome: "passed" | "failed" | "explained" | "skipped";
  commit_sha: string;
  branch: string;
  ci_run_id: string;
  ci_run_attempt: number;
  ci_job_id: string;
  url: string;
  runner_name: string;
  runner_labels: string[];
  started_at: string | null;
  completed_at: string | null;
  created_at: string;
  explained_by: ExplainingTest[];
  explained_by_jobs: ExplainingJob[]; // failed sibling Jobs; only a summary Job's explained executions
}

export interface RunnerStat {
  labels: string[]; // sorted label set
  runner_name: string | null; // null when grouped by label set only; a name appears on 2+ executions
  executions: number;
  unexplained_failures: number;
}

export interface JobHistory {
  job: Job;
  unexplained_failures: number;
  explained_failures: number;
  executions: JobExecution[];
  runners: RunnerStat[]; // over the newest score_window executions; worst unexplained rate first
  score_history: ScorePoint[];
}

export interface JobQuery {
  repo: string | null;
  page: number;
  pageSize: number;
  sort: SortKey;
  showStable: boolean;
  /** Exact Pipeline name to list, or null for every pipeline. */
  pipeline: string | null;
  /** List only acknowledged Jobs, sent as `acknowledged=true`. */
  acknowledged: boolean;
  /** Filter by Issue link, or null for any. */
  issue: IssueFilter | null;
  /** List only Jobs that became flaky in the last N days, or null for any. */
  newWithinDays: number | null;
  q: string;
}

export function fetchJobs(q: JobQuery): Promise<JobPage> {
  // Jobs takes no project, so the scope param stays hand-written here, as does
  // paging. The filters come from the table, in the table's declaration order:
  // q, sort, include_stable, acknowledged, has_issue, pipeline,
  // new_within_days. The order carries no meaning.
  const params = new URLSearchParams();
  if (q.repo) params.set("repo", q.repo);
  params.set("page", String(q.page));
  params.set("page_size", String(q.pageSize));
  setWireParams(
    params,
    q,
    LIST_FILTERS.map((d) => d.wire?.jobs),
  );
  return getJson<JobPage>(withQuery("/api/jobs", params));
}

export const fetchJobSummary = (repo: string | null) =>
  getJson<JobSummary>(
    withQuery("/api/jobs/summary", repo ? new URLSearchParams({ repo }) : new URLSearchParams()),
  );

export const fetchJobHistory = (id: number) =>
  getJson<JobHistory>(`/api/jobs/${id}/history?limit=60`);

/** `reason` and `expires_in_days` only mean something when quarantining; the backend
 *  clears both on un-quarantine, so leaving them out sends nothing new. */
export async function setQuarantine(
  id: number,
  quarantined: boolean,
  opts?: { reason?: string | null; expiresInDays?: number | null },
): Promise<TestCase> {
  const body: Record<string, boolean | string | number> = { quarantined };
  if (opts?.reason != null) body.reason = opts.reason;
  if (opts?.expiresInDays != null) body.expires_in_days = opts.expiresInDays;
  const resp = await fetch(`/api/tests/${id}/quarantine`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!resp.ok) throw new ApiError(`quarantine ${id} -> ${resp.status}`, resp.status);
  return resp.json() as Promise<TestCase>;
}

/** `reason` and `days` only mean something when acknowledging; the backend clears
 *  both on an un-acknowledge, so leaving them out sends nothing new. */
export async function setAcknowledge(
  kind: "tests" | "jobs",
  id: number,
  acknowledged: boolean,
  opts?: { reason?: string | null; days?: number | null },
): Promise<TestCase | Job> {
  const body: Record<string, boolean | string | number> = { acknowledged };
  if (opts?.reason != null) body.reason = opts.reason;
  if (opts?.days != null) body.days = opts.days;
  const resp = await fetch(`/api/${kind}/${id}/acknowledge`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!resp.ok) throw new ApiError(`acknowledge ${kind} ${id} -> ${resp.status}`, resp.status);
  return resp.json() as Promise<TestCase | Job>;
}

// --- Report queue (task 13) ----------------------------------------------

export interface ReportInfo {
  id: number;
  kind: "junit" | "pipeline";
  repo: string;
  project: string | null;
  commit_sha: string;
  branch: string;
  ci_run_id: string;
  status: "pending" | "processed" | "failed";
  error: string | null;
  counts: Record<string, number> | null;
  run_id: number | null;
  created_at: string;
  processed_at: string | null;
}

export interface ReportSummary {
  pending: number;
  failed: number;
}

export const fetchReportSummary = () => getJson<ReportSummary>("/api/reports/summary");

export const fetchFailedReports = () =>
  getJson<ReportInfo[]>("/api/reports?status=failed&limit=20");

// --- CI run failures (issue #26) -----------------------------------------

export interface ReportCounts {
  pending: number;
  processed: number;
  failed: number;
}

/** One failing (Test, Run) pair of a CI run attempt. */
export interface CiRunTestFailure {
  test_id: number;
  project: string;
  suite: string;
  classname: string;
  name: string;
  run_id: number;
  ci_job_id: string | null;
  /** Whether the Run this failure came from is suspected infrastructure, so scoring leaves it out (ADR 0013). */
  infra_suspect: boolean;
  /** The Test's last Execution in this Run: failed | error | passed (on retry) | skipped. */
  status: string;
  /** Failed + error Executions of this Test in this Run. */
  failed_attempts: number;
  failure_category: FailureCategory | null;
  message: string;
  tier: Tier;
  flakiness_score: number;
  confirmed_flake_count: number;
  clean_streak: number;
  /** Consecutive failing Default-branch Executions, newest first. */
  fail_streak: number;
  broken: boolean;
  quarantined: boolean;
  github_issue_number: number | null;
  github_issue_url: string | null;
  github_issue_closed: boolean;
}

export interface CiRunFailedJob {
  job_id: number;
  pipeline: string;
  name: string;
  ci_job_id: string;
  url: string;
  outcome: "explained" | "unexplained";
  /** A summary Job is explained by a failed non-summary Job, not by a test. */
  summary: boolean;
  tier: Tier;
  flakiness_score: number;
  confirmed_flake_count: number;
  clean_streak: number;
  github_issue_number: number | null;
  github_issue_url: string | null; // only when the Pipeline's provider is github
  github_issue_closed: boolean;
}

export interface CiRunFailures {
  repo: string;
  ci_run_id: string;
  ci_run_attempt: number;
  flake_threshold: number;
  /** Both Report kinds, this CI run attempt only. */
  reports: ReportCounts;
  /** (Test, Run) pairs before the cap. */
  total_failures: number;
  truncated: boolean;
  failures: CiRunTestFailure[];
  failed_jobs: CiRunFailedJob[];
}

/** `attempt` omitted or null means the latest attempt FlakeRadar has reports for. */
export const fetchCiRunFailures = (repo: string, ciRun: string, attempt?: number | null) => {
  const params = new URLSearchParams({ repo });
  if (attempt != null) params.set("ci_run_attempt", String(attempt));
  return getJson<CiRunFailures>(
    withQuery(`/api/ci-runs/${encodeURIComponent(ciRun)}/failures`, params),
  );
};
