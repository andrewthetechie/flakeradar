// Typed contract with the FastAPI backend (mirrors backend/app/schemas.py).

export type Tier = "flaky" | "suspect" | "stable";
export type SortKey = "score" | "last_seen" | "proven";

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
  last_status: string;
  last_seen_at: string;
  quarantined: boolean;
  quarantined_at: string | null;
  github_issue_number: number | null;
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
}

export interface Summary {
  total_tests: number;
  flaky_tests: number;
  suspect_tests: number;
  confirmed_flaky_tests: number;
  total_runs: number;
  total_executions: number;
  flake_threshold: number;
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
  return getJson<TestPage>(withQuery("/api/tests", params));
}

export const fetchHistory = (id: number) => getJson<History>(`/api/tests/${id}/history?limit=60`);

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
  repo: string;
  project: string;
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
