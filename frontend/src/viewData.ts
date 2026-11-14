// What each dashboard view fetches, in one framework-free module beside api.ts.
// A view's whole bundle — repos, tiles summary, page, queue — arrives from one
// call, so "what the Jobs view needs" lives here rather than in a branch of the
// component's refresh. The caller keeps the clock and the React lifecycle: it
// hands in the latestWins guard and the token it started, and this module answers
// nothing once a newer request superseded it. Errors propagate uncaught for the
// caller's single catch to map, exactly as the fetchers in api.ts do.
import {
  fetchHistory,
  fetchJobHistory,
  fetchJobSummary,
  fetchJobs,
  fetchReportSummary,
  fetchRepos,
  fetchSummary,
  fetchTests,
  type History,
  type JobHistory,
  type JobQuery,
  type JobPage,
  type JobSummary,
  type LatestWins,
  type RepoInfo,
  type ReportSummary,
  type Summary,
  type TestPage,
  type TestQuery,
} from "./api";
import { LIST_FILTERS, type ViewState } from "./urlState";

/** Everything the Tests view renders from one load. */
export interface TestsBundle {
  kind: "tests";
  repos: RepoInfo[];
  summary: Summary;
  page: TestPage;
  queue: ReportSummary;
}

/** Everything the Jobs view renders from one load. */
export interface JobsBundle {
  kind: "jobs";
  repos: RepoInfo[];
  jobSummary: JobSummary;
  jobPage: JobPage;
  queue: ReportSummary;
}

/**
 * Everything the CI run view renders from one load. CiRunView fetches the run
 * itself, so the header bundle stays repos + queue.
 */
export interface CiRunBundle {
  kind: "ci-run";
  repos: RepoInfo[];
  queue: ReportSummary;
}

export type ViewBundle = TestsBundle | JobsBundle | CiRunBundle;

/**
 * The filters a load consumes: a Pick of the fields ViewState already carries,
 * so a view never needs a second copy of its own filters.
 */
export type ViewFilters = Pick<
  ViewState,
  | "view"
  | "repo"
  | "project"
  | "page"
  | "sort"
  | "showStable"
  | "cause"
  | "quarantined"
  | "broken"
  | "issue"
  | "newDays"
  | "pipeline"
  | "q"
>;

/** Every leaderboard page loads at this size. */
const PAGE_SIZE = 50;

/** The two fetch-argument names the table cannot carry, because `api.ts` spells
 *  them differently from the URL and from the wire: `cause` goes into a query as
 *  `category`, `newDays` as `newWithinDays` (`TestQuery`, `JobQuery`). Every other
 *  filter enters the query under its ViewState field name. */
const QUERY_NAMES: Partial<Record<keyof ViewState, keyof TestQuery | keyof JobQuery>> = {
  cause: "category",
  newDays: "newWithinDays",
};

/** What a caller hands `queryFor` besides the filters: the scope and paging the
 *  table does not describe. `project` is optional because the Jobs query has none. */
interface QueryBase {
  repo: string | null;
  project?: string | null;
  page: number;
  pageSize: number;
}

/**
 * The fetch argument for one endpoint: the scope and paging spelled by hand, plus
 * every list filter the declaration maps onto that endpoint, read out of the
 * view's filters. So the Jobs query drops project and the tests-only filters, and
 * the Tests query drops pipeline, because the table says so rather than because
 * this module enumerates them — and a filter added to the table reaches the
 * endpoints it declares without an edit here. The result is the table's shape, not
 * the literal's, which is why it is cast rather than checked.
 */
function queryFor<Q>(endpoint: "tests" | "jobs", f: ViewFilters, base: QueryBase): Q {
  const query: Record<string, unknown> = { ...base };
  for (const d of LIST_FILTERS) {
    // A wire-mapped field is by construction a filter a load consumes, so the
    // read is one `keyof ViewFilters` narrowing of the table's wider field key.
    if (d.wire?.[endpoint]) {
      query[QUERY_NAMES[d.field] ?? d.field] = f[d.field as keyof ViewFilters];
    }
  }
  return query as Q;
}

// Each loader begins the queue request first, so the one request per load that
// every view awaits overlaps the rest instead of adding a round trip.

/** The Tests view's bundle: scope tiles, leaderboard page, queue. */
export async function loadTestsBundle(
  f: ViewFilters,
  guard: LatestWins,
  request: number,
): Promise<TestsBundle | null> {
  const scope = { repo: f.repo, project: f.project };
  const queueQ = fetchReportSummary();
  const [repos, summary, page, queue] = await Promise.all([
    fetchRepos(),
    fetchSummary(scope),
    fetchTests(queryFor<TestQuery>("tests", f, { ...scope, page: f.page, pageSize: PAGE_SIZE })),
    queueQ,
  ]);
  if (!guard.current(request)) return null;
  return { kind: "tests", repos, summary, page, queue };
}

/** The Jobs view's bundle: job tiles, job page, queue. */
export async function loadJobsBundle(
  f: ViewFilters,
  guard: LatestWins,
  request: number,
): Promise<JobsBundle | null> {
  const queueQ = fetchReportSummary();
  const [repos, jobSummary, jobPage, queue] = await Promise.all([
    fetchRepos(),
    fetchJobSummary(f.repo),
    // Jobs takes no project, so the scope is just the repo (`JobQuery` has no
    // project field to write one into).
    fetchJobs(queryFor<JobQuery>("jobs", f, { repo: f.repo, page: f.page, pageSize: PAGE_SIZE })),
    queueQ,
  ]);
  if (!guard.current(request)) return null;
  return { kind: "jobs", repos, jobSummary, jobPage, queue };
}

/**
 * The CI run view's bundle: the scope picker's repos and the queue. CiRunView
 * fetches the run itself, so this view's filters go unread; the signature stays
 * uniform so one call site can hand the same arguments to every loader.
 */
export async function loadCiRunBundle(
  _f: ViewFilters,
  guard: LatestWins,
  request: number,
): Promise<CiRunBundle | null> {
  const queueQ = fetchReportSummary();
  const [repos, queue] = await Promise.all([fetchRepos(), queueQ]);
  if (!guard.current(request)) return null;
  return { kind: "ci-run", repos, queue };
}

/**
 * The load of whichever view is current. Dispatching here keeps the caller free of
 * per-kind endpoint knowledge; the returned union narrows on `kind`.
 */
export function loadViewBundle(
  f: ViewFilters,
  guard: LatestWins,
  request: number,
): Promise<ViewBundle | null> {
  if (f.view === "jobs") return loadJobsBundle(f, guard, request);
  if (f.view === "ci-run") return loadCiRunBundle(f, guard, request);
  return loadTestsBundle(f, guard, request);
}

/** A Test drawer's history, or null once a newer drawer request superseded it. */
export async function loadTestHistory(
  id: number,
  guard: LatestWins,
  request: number,
): Promise<History | null> {
  const history = await fetchHistory(id);
  if (!guard.current(request)) return null;
  return history;
}

/** A Job drawer's history, or null once a newer drawer request superseded it. */
export async function loadJobHistory(
  id: number,
  guard: LatestWins,
  request: number,
): Promise<JobHistory | null> {
  const history = await fetchJobHistory(id);
  if (!guard.current(request)) return null;
  return history;
}
