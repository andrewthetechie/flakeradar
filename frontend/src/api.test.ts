import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, fetchRepos, setQuarantine } from "./api";

interface Post {
  path: string;
  body: Record<string, unknown>;
}

/** Stubs the network and records every POST the client makes. */
function stubFetch(): Post[] {
  const posts: Post[] = [];
  vi.stubGlobal("fetch", async (input: string | URL, init?: RequestInit) => {
    posts.push({ path: String(input), body: JSON.parse(String(init?.body)) as Post["body"] });
    return { ok: true, status: 200, json: async () => ({}) } as Response;
  });
  return posts;
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
    const posts = stubFetch();
    await setQuarantine(7, true, { reason: "flaky on Windows", expiresInDays: 14 });
    expect(posts).toEqual([
      {
        path: "/api/tests/7/quarantine",
        body: { quarantined: true, reason: "flaky on Windows", expires_in_days: 14 },
      },
    ]);
  });

  it("omits the empty choices so an un-quarantine stays a plain toggle", async () => {
    const posts = stubFetch();
    await setQuarantine(7, false, { reason: null, expiresInDays: null });
    expect(posts[0]?.body).toEqual({ quarantined: false });
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
