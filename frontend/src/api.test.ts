import { afterEach, describe, expect, it, vi } from "vitest";
import { setQuarantine } from "./api";

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
