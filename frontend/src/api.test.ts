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
  setAcknowledge,
  setQuarantine,
} from "./api";

interface Call {
  path: string;
  method?: string;
  body?: Record<string, unknown>;
}

/** Stubs the network and records every request the client makes. */
function stubFetch(): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", async (input: string | URL, init?: RequestInit) => {
    calls.push({
      path: String(input),
      method: init?.method,
      body: init?.body == null ? undefined : (JSON.parse(String(init.body)) as Call["body"]),
    });
    return { ok: true, status: 200, json: async () => ({}) } as Response;
  });
  return calls;
}

/** The endpoint and the parsed query parameters of the one request a builder
 *  made: the parameter set and its values are the contract, not the order the
 *  code appended them in. */
function request(calls: Call[]): { path: string; params: Record<string, string> } {
  expect(calls).toHaveLength(1);
  const raw = calls[0]?.path ?? "";
  const i = raw.indexOf("?");
  return {
    path: i < 0 ? raw : raw.slice(0, i),
    params: Object.fromEntries(new URLSearchParams(i < 0 ? "" : raw.slice(i + 1))),
  };
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
        method: "POST",
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

describe("setAcknowledge", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("posts the reason and the days to the kind's endpoint", async () => {
    const calls = stubFetch();
    await setAcknowledge("jobs", 3, true, { reason: "runner pool", days: 7 });
    expect(calls).toEqual([
      {
        path: "/api/jobs/3/acknowledge",
        method: "POST",
        body: { acknowledged: true, reason: "runner pool", days: 7 },
      },
    ]);
  });

  it("omits the empty choices so a clear stays a plain toggle", async () => {
    const calls = stubFetch();
    await setAcknowledge("tests", 7, false);
    expect(calls[0]?.body).toEqual({ acknowledged: false });
  });

  it("rejects a 422 with the status the backend answered", async () => {
    stubFailure(422);
    const e = await setAcknowledge("tests", 7, true, { reason: "flaky", days: 99 }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(e).toBeInstanceOf(ApiError);
    expect((e as ApiError).status).toBe(422);
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
 * Every GET builder is a parameter mapping, so the contract under test is the
 * complete set of parameters and their values; the order the code appends them
 * in is not part of it.
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
      acknowledged: false,
      broken: false,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/tests",
      params: {
        repo: "a/b",
        page: "2",
        page_size: "50",
        sort: "proven",
        include_stable: "true",
        category: "timing",
      },
    });
  });

  it("fetchTests sends the broken filter as broken=true", async () => {
    const calls = stubFetch();
    await fetchTests({
      repo: "a/b",
      project: null,
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: false,
      category: null,
      quarantined: false,
      acknowledged: false,
      broken: true,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/tests",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "score", broken: "true" },
    });
  });

  // Every filter the Tests endpoint takes, set at once. The map is the recorded
  // set, parsed: the whole-URL assertion this case replaced, plus the
  // acknowledged=true the separate acknowledged case recorded (the table's entry
  // reads only that field, so the two sets simply union). repo, page, page_size,
  // sort, include_stable, category, quarantined, broken, has_issue,
  // new_within_days, q, acknowledged: nothing else may appear, none may be missing.
  it("fetchTests maps every filter through the table", async () => {
    const calls = stubFetch();
    await fetchTests({
      repo: "a/b",
      project: null,
      page: 1,
      pageSize: 50,
      sort: "proven",
      showStable: true,
      category: "timing",
      quarantined: true,
      acknowledged: true,
      broken: true,
      issue: "linked",
      newWithinDays: 7,
      q: "login",
    });
    expect(request(calls)).toEqual({
      path: "/api/tests",
      params: {
        repo: "a/b",
        page: "1",
        page_size: "50",
        sort: "proven",
        include_stable: "true",
        category: "timing",
        quarantined: "true",
        broken: "true",
        has_issue: "true",
        new_within_days: "7",
        q: "login",
        acknowledged: "true",
      },
    });
  });

  it("fetchTests sends issue=none as has_issue=false", async () => {
    const calls = stubFetch();
    await fetchTests({
      repo: "a/b",
      project: null,
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: false,
      category: null,
      quarantined: false,
      acknowledged: false,
      broken: false,
      issue: "none",
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/tests",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "score", has_issue: "false" },
    });
  });

  it("fetchTests sends acknowledged=true", async () => {
    const calls = stubFetch();
    await fetchTests({
      repo: "a/b",
      project: null,
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: false,
      category: null,
      quarantined: false,
      acknowledged: true,
      broken: false,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/tests",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "score", acknowledged: "true" },
    });
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
      acknowledged: false,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/jobs",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "score", include_stable: "true" },
    });
  });

  // Jobs' table entries cover every filter this endpoint takes, all set at once.
  // The map is the recorded set, parsed: the whole-URL assertion this case
  // replaced, plus the acknowledged=true the separate acknowledged case recorded
  // (the table's entry reads only that field, so the two sets simply union).
  // repo, page, page_size, sort, include_stable, q, pipeline, has_issue,
  // new_within_days, acknowledged: nothing else may appear, none may be missing.
  it("fetchJobs maps every Jobs filter through the table", async () => {
    const calls = stubFetch();
    await fetchJobs({
      repo: "a/b",
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: true,
      pipeline: "ci.yml",
      acknowledged: true,
      issue: "none",
      newWithinDays: 7,
      q: "login",
    });
    expect(request(calls)).toEqual({
      path: "/api/jobs",
      params: {
        repo: "a/b",
        page: "1",
        page_size: "50",
        sort: "score",
        include_stable: "true",
        q: "login",
        pipeline: "ci.yml",
        has_issue: "false",
        new_within_days: "7",
        acknowledged: "true",
      },
    });
  });

  it("fetchJobs sends issue=linked as has_issue=true", async () => {
    const calls = stubFetch();
    await fetchJobs({
      repo: "a/b",
      page: 1,
      pageSize: 50,
      sort: "proven",
      showStable: false,
      pipeline: null,
      acknowledged: false,
      issue: "linked",
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/jobs",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "proven", has_issue: "true" },
    });
  });

  it("fetchJobs sends acknowledged=true", async () => {
    const calls = stubFetch();
    await fetchJobs({
      repo: "a/b",
      page: 1,
      pageSize: 50,
      sort: "score",
      showStable: false,
      pipeline: null,
      acknowledged: true,
      issue: null,
      newWithinDays: null,
      q: "",
    });
    expect(request(calls)).toEqual({
      path: "/api/jobs",
      params: { repo: "a/b", page: "1", page_size: "50", sort: "score", acknowledged: "true" },
    });
  });

  it("fetchFailedReports asks for the failed reports window", async () => {
    const calls = stubFetch();
    await fetchFailedReports();
    expect(calls.map((c) => c.path)).toEqual(["/api/reports?status=failed&limit=20"]);
  });
});
