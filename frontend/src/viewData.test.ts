// The data contract of each dashboard view, tested without a component: the
// seam is the global fetch stub, exactly as in api.test.ts, so the exact URL set
// one view loads — and the bundle it answers with — is checkable here directly.
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, latestWins } from "./api";
import { DEFAULT_VIEW } from "./urlState";
import {
  loadCiRunBundle,
  loadJobHistory,
  loadJobsBundle,
  loadTestHistory,
  loadTestsBundle,
  loadViewBundle,
  type ViewFilters,
} from "./viewData";

/** The filters a load reads. Everything else comes from the view defaults. */
const filters = (over: Partial<ViewFilters> = {}): ViewFilters => ({
  ...DEFAULT_VIEW,
  repo: "a/b",
  project: "backend",
  page: 2,
  ...over,
});

const QUEUE = "/api/reports/summary";

const reposFixture = [{ name: "a/b", projects: [{ name: "backend", root: "." }] }];
const summaryFixture = { total_tests: 3, flaky_tests: 1 };
const testPageFixture = { items: [{ id: 7, name: "test_a" }], total: 1, page: 2, page_size: 50 };
const jobSummaryFixture = { total_jobs: 2, flaky_jobs: 1 };
const jobPageFixture = { items: [{ id: 9, name: "job_a" }], total: 1, page: 2, page_size: 50 };
const queueFixture = { pending: 2, failed: 1 };
const historyFixture = { test: { id: 7, name: "test_a" }, executions: [] };
const jobHistoryFixture = { job: { id: 9, name: "job_a" }, executions: [] };

/** The canned body of each endpoint, keyed by path with the query stripped. */
const BODIES: Record<string, unknown> = {
  "/api/repos": reposFixture,
  "/api/summary": summaryFixture,
  "/api/tests": testPageFixture,
  "/api/jobs/summary": jobSummaryFixture,
  "/api/jobs": jobPageFixture,
  [QUEUE]: queueFixture,
  "/api/tests/7/history": historyFixture,
  "/api/jobs/9/history": jobHistoryFixture,
};

/**
 * Stubs the network, recording every requested URL and answering each path with
 * its canned body. The paths in `failPaths` answer with a failed response.
 */
function stubFetch(failPaths: readonly string[] = []): string[] {
  const urls: string[] = [];
  vi.stubGlobal("fetch", async (input: string | URL) => {
    const url = String(input);
    urls.push(url);
    const path = url.split("?")[0];
    if (failPaths.includes(path)) {
      return { ok: false, status: 503, json: async () => ({}) } as unknown as Response;
    }
    return { ok: true, status: 200, json: async () => BODIES[path] ?? {} } as unknown as Response;
  });
  return urls;
}

/** The queue request is one per load, however many fetchers the view has. */
function queueRequests(urls: readonly string[]): string[] {
  return urls.filter((url) => url === QUEUE);
}

/** A loader answers `null` only when superseded; these tests want the bundle. */
async function unwrap<T>(promise: Promise<T | null>): Promise<T> {
  const value = await promise;
  if (value == null) throw new Error("expected a bundle, the loader answered null");
  return value;
}

afterEach(() => vi.unstubAllGlobals());

describe("loadTestsBundle", () => {
  it("issues exactly the Tests endpoints and returns the Tests bundle", async () => {
    const urls = stubFetch();
    const guard = latestWins();
    const bundle = await unwrap(loadTestsBundle(filters(), guard, guard.start()));

    expect([...urls].sort()).toEqual(
      [
        "/api/repos",
        "/api/summary?repo=a%2Fb&project=backend",
        "/api/tests?repo=a%2Fb&project=backend&page=2&page_size=50&sort=score",
        QUEUE,
      ].sort(),
    );
    expect(queueRequests(urls)).toHaveLength(1);
    expect(bundle).toEqual({
      kind: "tests",
      repos: reposFixture,
      summary: summaryFixture,
      page: testPageFixture,
      queue: queueFixture,
    });
  });
});

describe("loadJobsBundle", () => {
  it("issues exactly the Jobs endpoints and returns the Jobs bundle", async () => {
    const urls = stubFetch();
    const guard = latestWins();
    const bundle = await unwrap(loadJobsBundle(filters({ view: "jobs" }), guard, guard.start()));

    expect([...urls].sort()).toEqual(
      [
        "/api/repos",
        "/api/jobs/summary?repo=a%2Fb",
        "/api/jobs?repo=a%2Fb&page=2&page_size=50&sort=score",
        QUEUE,
      ].sort(),
    );
    expect(queueRequests(urls)).toHaveLength(1);
    expect(bundle).toEqual({
      kind: "jobs",
      repos: reposFixture,
      jobSummary: jobSummaryFixture,
      jobPage: jobPageFixture,
      queue: queueFixture,
    });
  });
});

describe("loadCiRunBundle", () => {
  it("issues the repos and queue requests, and nothing else", async () => {
    const urls = stubFetch();
    const guard = latestWins();
    const bundle = await unwrap(loadCiRunBundle(filters({ view: "ci-run" }), guard, guard.start()));

    expect([...urls].sort()).toEqual(["/api/repos", QUEUE].sort());
    expect(queueRequests(urls)).toHaveLength(1);
    expect(bundle).toEqual({ kind: "ci-run", repos: reposFixture, queue: queueFixture });
  });
});

describe("loadViewBundle", () => {
  it("dispatches on the view, and the bundle narrows on kind", async () => {
    const urls = stubFetch();
    const guard = latestWins();

    const tests = await unwrap(loadViewBundle(filters(), guard, guard.start()));
    expect(tests.kind).toBe("tests");
    if (tests.kind === "tests") expect(tests.page).toEqual(testPageFixture);

    const jobs = await unwrap(loadViewBundle(filters({ view: "jobs" }), guard, guard.start()));
    expect(jobs.kind).toBe("jobs");
    if (jobs.kind === "jobs") expect(jobs.jobPage).toEqual(jobPageFixture);

    urls.length = 0;
    const ciRun = await unwrap(loadViewBundle(filters({ view: "ci-run" }), guard, guard.start()));
    expect(ciRun.kind).toBe("ci-run");
    if (ciRun.kind === "ci-run") expect(ciRun.queue).toEqual(queueFixture);
    // Dispatching means asking only what that view renders: no leaderboard query.
    expect([...urls].sort()).toEqual(["/api/repos", QUEUE].sort());
  });
});

describe("a superseded answer", () => {
  it("writes nothing: every loader resolves null", async () => {
    stubFetch();
    const guard = latestWins();

    const stale = guard.start();
    guard.start(); // a newer request begins before any of these answers
    await expect(loadTestsBundle(filters(), guard, stale)).resolves.toBeNull();
    await expect(loadJobsBundle(filters({ view: "jobs" }), guard, stale)).resolves.toBeNull();
    await expect(loadCiRunBundle(filters({ view: "ci-run" }), guard, stale)).resolves.toBeNull();
    await expect(loadViewBundle(filters(), guard, stale)).resolves.toBeNull();
    await expect(loadTestHistory(7, guard, stale)).resolves.toBeNull();
    await expect(loadJobHistory(9, guard, stale)).resolves.toBeNull();
  });
});

describe("a rejected request", () => {
  it("propagates uncaught, so the caller's single catch maps it", async () => {
    stubFetch(["/api/tests"]);
    const guard = latestWins();

    const error = await loadTestsBundle(filters(), guard, guard.start()).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).message).toBe(
      "/api/tests?repo=a%2Fb&project=backend&page=2&page_size=50&sort=score -> 503",
    );
  });

  it("propagates out of a drawer loader too", async () => {
    stubFetch(["/api/jobs/9/history"]);
    const guard = latestWins();

    await expect(loadJobHistory(9, guard, guard.start())).rejects.toBeInstanceOf(ApiError);
  });

  it("propagates a network rejection, not only a failed response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL) => {
        if (String(input) === QUEUE) return new Response(JSON.stringify(queueFixture));
        throw new TypeError("Failed to fetch");
      }),
    );
    const guard = latestWins();

    await expect(
      loadJobsBundle(filters({ view: "jobs" }), guard, guard.start()),
    ).rejects.toBeInstanceOf(TypeError);
  });
});

describe("drawer loaders", () => {
  it("loadTestHistory asks for the one Test's history window", async () => {
    const urls = stubFetch();
    const guard = latestWins();

    const history = await unwrap(loadTestHistory(7, guard, guard.start()));
    expect(urls).toEqual(["/api/tests/7/history?limit=60"]);
    expect(history).toEqual(historyFixture);
  });

  it("loadJobHistory asks for the one Job's history window", async () => {
    const urls = stubFetch();
    const guard = latestWins();

    const history = await unwrap(loadJobHistory(9, guard, guard.start()));
    expect(urls).toEqual(["/api/jobs/9/history?limit=60"]);
    expect(history).toEqual(jobHistoryFixture);
  });
});
