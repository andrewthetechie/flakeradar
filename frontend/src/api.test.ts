import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  fetchFailedReports,
  fetchHistory,
  fetchJobs,
  fetchRepos,
  fetchTests,
  latestWins,
  scopeParams,
  setQuarantine,
} from "./api";

interface Call {
  path: string;
  body?: Record<string, unknown>;
}

/** Stubs the network and records every request the client makes. */
function stubFetch(): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", async (input: string | URL, init?: RequestInit) => {
    calls.push({
      path: String(input),
      body: init?.body == null ? undefined : (JSON.parse(String(init.body)) as Call["body"]),
    });
    return { ok: true, status: 200, json: async () => ({}) } as Response;
  });
  return calls;
}

/** Stubs the network so every request answers with a failed response. */
function stubFailure(status: number): void {
  vi.stubGlobal(
    "fetch",
    async () => ({ ok: false, status, json: async () => ({}) }) as unknown as Response,
  );
}

describe("setQuarantine", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("posts the reason and the expiry when they are given", async () => {
    const calls = stubFetch();
    await setQuarantine(7, true, { reason: "flaky on Windows", expiresInDays: 14 });
    expect(calls).toEqual([
      {
        path: "/api/tests/7/quarantine",
        body: { quarantined: true, reason: "flaky on Windows", expires_in_days: 14 },
      },
    ]);
  });

  it("omits the empty choices so an un-quarantine stays a plain toggle", async () => {
    const calls = stubFetch();
    await setQuarantine(7, false, { reason: null, expiresInDays: null });
    expect(calls[0]?.body).toEqual({ quarantined: false });
  });
});

/**
 * getJson is module-private, so its rejection is exercised through an exported
 * fetcher. The wording is part of the contract: the error banners render it.
 */
describe("rejected ApiError", () => {
  afterEach(() => vi.unstubAllGlobals());

  async function rejection(promise: Promise<unknown>): Promise<unknown> {
    return promise.then(
      () => undefined,
      (e: unknown) => e,
    );
  }

  it("exposes the status of a 404 through a GET fetcher", async () => {
    stubFailure(404);
    const e = await rejection(fetchRepos());
    expect(e).toBeInstanceOf(ApiError);
    expect((e as ApiError).status).toBe(404);
    expect((e as Error).message).toBe("/api/repos -> 404");
  });

  it("exposes the status of a 500 through a GET fetcher", async () => {
    stubFailure(500);
    const e = await rejection(fetchRepos());
    expect(e).toBeInstanceOf(ApiError);
    expect((e as ApiError).status).toBe(500);
    expect((e as Error).message).toBe("/api/repos -> 500");
  });

  it("carries the status out of the POST too", async () => {
    stubFailure(500);
    const e = await rejection(setQuarantine(7, true));
    expect(e).toBeInstanceOf(ApiError);
    expect((e as ApiError).status).toBe(500);
    expect((e as Error).message).toBe("quarantine 7 -> 500");
  });
});

/**
 * The newest-request-wins guard, tested without a component: the point of the
 * helper is that "a late answer never writes state" is checkable here rather
 * than through a full-App render.
 */
describe("latestWins", () => {
  it("lets the newest request write", () => {
    const guard = latestWins();
    expect(guard.current(guard.start())).toBe(true);
  });

  it("discards an answer superseded by a newer request", () => {
    const guard = latestWins();
    const first = guard.start();
    guard.start(); // a newer request begins
    expect(guard.current(first)).toBe(false); // the late answer must not write
  });

  it("cancel() discards every outstanding answer", () => {
    const guard = latestWins();
    const older = guard.start();
    const newer = guard.start();
    guard.cancel();
    expect(guard.current(older)).toBe(false);
    expect(guard.current(newer)).toBe(false);
  });

  it("accepts a fresh request after cancel()", () => {
    const guard = latestWins();
    guard.start();
    guard.cancel();
    expect(guard.current(guard.start())).toBe(true);
  });
});

/**
 * The scope a Test list is built from: a project only means something with its
 * repo, so a project without one has to disappear from the query.
 */
describe("scopeParams", () => {
  it("keeps the repo encoded and appends the project", () => {
    expect(scopeParams({ repo: "a/b", project: "backend" }).toString()).toBe(
      "repo=a%2Fb&project=backend",
    );
  });

  it("leaves the project out when there is none", () => {
    expect(scopeParams({ repo: "a/b", project: null }).toString()).toBe("repo=a%2Fb");
  });

  it("drops a project that has no repo", () => {
    expect(scopeParams({ repo: null, project: "backend" }).toString()).toBe("");
  });
});

/**
 * Every GET builder is a parameter mapping, so the exact URL — key names, values
 * and the order the code sets them in — is the contract under test.
 */
describe("GET request builders", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("fetchTests sends the scope, paging and every active filter", async () => {
    const calls = stubFetch();
    await fetchTests({
      repo: "a/b",
      project: null,
      page: 2,
      pageSize: 50,
      sort: "proven",
      showStable: true,
      category: "timing",
      quarantined: false,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(calls.map((c) => c.path)).toEqual([
      "/api/tests?repo=a%2Fb&page=2&page_size=50&sort=proven&include_stable=true&category=timing",
    ]);
  });

  it("fetchHistory asks for the fixed history window of one test", async () => {
    const calls = stubFetch();
    await fetchHistory(7);
    expect(calls.map((c) => c.path)).toEqual(["/api/tests/7/history?limit=60"]);
  });

  it("fetchJobs sends the paging, sort and stable toggle", async () => {
    const calls = stubFetch();
    await fetchJobs({
      repo: "a/b",
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: true,
      pipeline: null,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(calls.map((c) => c.path)).toEqual([
      "/api/jobs?repo=a%2Fb&page=1&page_size=50&sort=score&include_stable=true",
    ]);
  });

  it("fetchFailedReports asks for the failed reports window", async () => {
    const calls = stubFetch();
    await fetchFailedReports();
    expect(calls.map((c) => c.path)).toEqual(["/api/reports?status=failed&limit=20"]);
  });
});
