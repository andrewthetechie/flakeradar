// Typed contract with the FastAPI backend (mirrors backend/app/schemas.py).

export type Tier = "flaky" | "suspect" | "stable";
export type SortKey = "score" | "last_seen" | "proven";
export type FailureCategory = "network" | "environment" | "timing" | "assertion" | "other";
export const FAILURE_CATEGORIES: readonly FailureCategory[] = [
  "network",
  "environment",
  "timing",
  "assertion",
  "other",
];
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
  trend: Trend | null;
  last_status: string;
  last_seen_at: string;
  quarantined: boolean;
  quarantined_at: string | null;
  github_issue_number: number | null;
  github_issue_url: string | null;
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
  total_runs: number;
  total_executions: number;
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
}

async function getJson<T>(url: string): Promise<T> {
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`${url} -> ${resp.status}`);
  return resp.json() as Promise<T>;
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

export function fetchTests(q: TestQuery): Promise<TestPage> {
  const params = scopeParams(q);
  params.set("page", String(q.page));
  params.set("page_size", String(q.pageSize));
  params.set("sort", q.sort);
  if (q.showStable) params.set("include_stable", "true");
  if (q.category) params.set("category", q.category);
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
  github_issue_number: number | null;
  github_issue_url: string | null;
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
  total_job_executions: number;
  flake_threshold: number;
}

export interface ExplainingTest {
  test_id: number;
  project: string;
  classname: string;
  name: string;
  status: string;
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
}

export interface JobHistory {
  job: Job;
  unexplained_failures: number;
  explained_failures: number;
  executions: JobExecution[];
  score_history: ScorePoint[];
}

export interface JobQuery {
  repo: string | null;
  page: number;
  pageSize: number;
  sort: SortKey;
  showStable: boolean;
}

export function fetchJobs(q: JobQuery): Promise<JobPage> {
  const params = new URLSearchParams();
  if (q.repo) params.set("repo", q.repo);
  params.set("page", String(q.page));
  params.set("page_size", String(q.pageSize));
  params.set("sort", q.sort);
  if (q.showStable) params.set("include_stable", "true");
  return getJson<JobPage>(withQuery("/api/jobs", params));
}

export const fetchJobSummary = (repo: string | null) =>
  getJson<JobSummary>(
    withQuery("/api/jobs/summary", repo ? new URLSearchParams({ repo }) : new URLSearchParams()),
  );

export const fetchJobHistory = (id: number) =>
  getJson<JobHistory>(`/api/jobs/${id}/history?limit=60`);

export async function setQuarantine(id: number, quarantined: boolean): Promise<TestCase> {
  const resp = await fetch(`/api/tests/${id}/quarantine`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ quarantined }),
  });
  if (!resp.ok) throw new Error(`quarantine ${id} -> ${resp.status}`);
  return resp.json() as Promise<TestCase>;
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
