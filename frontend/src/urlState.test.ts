import { describe, expect, it } from "vitest";
import { DEFAULT_VIEW, applyPatch, parseViewState, serializeViewState } from "./urlState";

describe("view state in the URL", () => {
  it("round-trips a full view", () => {
    const view = {
      repo: "andrewthetechie/writers-app",
      project: "backend",
      test: 42,
      page: 3,
      sort: "proven" as const,
      showStable: true,
    };
    const search = serializeViewState(view);
    expect(search).toBe(
      "?repo=andrewthetechie%2Fwriters-app&project=backend&test=42&page=3&sort=proven&stable=1",
    );
    expect(parseViewState(search)).toEqual(view);
  });

  it("omits defaults and parses an empty query to the default view", () => {
    expect(serializeViewState(DEFAULT_VIEW)).toBe("");
    expect(parseViewState("")).toEqual(DEFAULT_VIEW);
  });

  it("ignores invalid values and a project without a repo", () => {
    expect(parseViewState("?project=backend&page=0&test=abc&sort=name&stable=yes")).toEqual(
      DEFAULT_VIEW,
    );
  });
});

describe("applyPatch", () => {
  const base = { ...DEFAULT_VIEW, repo: "a/b", project: "backend", page: 4, test: 7 };

  it("changing repo clears project and resets page, keeps the selected test", () => {
    expect(applyPatch(base, { repo: "c/d" })).toEqual({
      ...base,
      repo: "c/d",
      project: null,
      page: 1,
    });
  });

  it("sort and stable toggle reset page; selecting a test does not", () => {
    expect(applyPatch(base, { sort: "last_seen" }).page).toBe(1);
    expect(applyPatch(base, { showStable: true }).page).toBe(1);
    expect(applyPatch(base, { test: 9 }).page).toBe(4);
    expect(applyPatch(base, { page: 5 }).page).toBe(5);
  });

  it("clearing the repo clears the project", () => {
    expect(applyPatch(base, { repo: null, project: null })).toMatchObject({
      repo: null,
      project: null,
      page: 1,
    });
  });
});
