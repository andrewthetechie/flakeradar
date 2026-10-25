import { describe, expect, it } from "vitest";
import { DEFAULT_VIEW, applyPatch, parseViewState, serializeViewState } from "./urlState";

describe("view state in the URL", () => {
  it("round-trips a full view", () => {
    const view = {
      repo: "andrewthetechie/writers-app",
      project: "backend",
      test: 42,
      job: null,
      page: 3,
      sort: "proven" as const,
      showStable: true,
      cause: null,
      quarantined: false,
      issue: null,
      view: "tests" as const,
      ci_run: null,
      attempt: null,
      q: "",
    };
    const search = serializeViewState(view);
    expect(search).toBe(
      "?repo=andrewthetechie%2Fwriters-app&project=backend&test=42&page=3&sort=proven&stable=1",
    );
    expect(parseViewState(search)).toEqual(view);
  });

  it("serializes and parses the jobs view and a job id", () => {
    const view = {
      repo: "andrewthetechie/writers-app",
      project: "backend",
      test: null,
      job: 3,
      page: 2,
      sort: "score" as const,
      showStable: false,
      cause: null,
      quarantined: false,
      issue: null,
      view: "jobs" as const,
      ci_run: null,
      attempt: null,
      q: "",
    };
    const search = serializeViewState(view);
    expect(search).toBe(
      "?repo=andrewthetechie%2Fwriters-app&project=backend&view=jobs&job=3&page=2",
    );
    expect(parseViewState(search)).toEqual(view);
  });

  it("serializes and parses the ci-run view with its run id and attempt", () => {
    const view = {
      repo: "a/b",
      project: null,
      test: null,
      job: null,
      page: 1,
      sort: "score" as const,
      showStable: false,
      cause: null,
      quarantined: false,
      issue: null,
      view: "ci-run" as const,
      ci_run: "12345678",
      attempt: 2,
      q: "",
    };
    expect(serializeViewState(view)).toBe("?repo=a%2Fb&view=ci-run&ci_run=12345678&attempt=2");
    expect(parseViewState("?repo=a%2Fb&view=ci-run&ci_run=12345678&attempt=2")).toEqual(view);
    // The attempt defaults to the latest one, so it is omitted when unset, and
    // the CI run parameters never appear in the other views.
    expect(serializeViewState({ ...view, attempt: null })).toBe(
      "?repo=a%2Fb&view=ci-run&ci_run=12345678",
    );
    expect(parseViewState("?repo=a%2Fb&view=ci-run&ci_run=12345678").attempt).toBeNull();
    expect(serializeViewState({ ...DEFAULT_VIEW, repo: "a/b", ci_run: "9", attempt: 3 })).toBe(
      "?repo=a%2Fb",
    );
    expect(parseViewState("?view=ci-run")).toMatchObject({ view: "ci-run", ci_run: null });
  });

  it("round-trips a search term, omitted when empty", () => {
    const view = { ...DEFAULT_VIEW, q: "login", page: 2 };
    expect(serializeViewState(view)).toBe("?q=login&page=2");
    expect(parseViewState("?q=login&page=2")).toEqual(view);
    expect(serializeViewState({ ...DEFAULT_VIEW, q: "" })).toBe("");
    expect(parseViewState("?q=").q).toBe("");
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

  it("parses, validates and serializes cause", () => {
    expect(parseViewState("?cause=timing").cause).toBe("timing");
    expect(parseViewState("?cause=nope").cause).toBeNull();
    expect(serializeViewState({ ...DEFAULT_VIEW, showStable: true, cause: "network" })).toBe(
      "?stable=1&cause=network",
    );
  });

  it("round-trips the quarantined and issue filters", () => {
    const view = { ...DEFAULT_VIEW, repo: "a/b", quarantined: true, issue: "linked" as const };
    expect(serializeViewState(view)).toBe("?repo=a%2Fb&quarantined=1&issue=linked");
    expect(parseViewState("?repo=a%2Fb&quarantined=1&issue=linked")).toEqual(view);
    expect(serializeViewState({ ...DEFAULT_VIEW, issue: "none" })).toBe("?issue=none");
  });

  it("ignores an invalid issue filter and quarantined values other than 1", () => {
    expect(parseViewState("?issue=bogus").issue).toBeNull();
    expect(parseViewState("?issue=true").issue).toBeNull();
    expect(parseViewState("?quarantined=0").quarantined).toBe(false);
    expect(parseViewState("?quarantined=true").quarantined).toBe(false);
    expect(parseViewState("").quarantined).toBe(false);
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
    expect(applyPatch(base, { cause: "timing" }).page).toBe(1);
    expect(applyPatch(base, { quarantined: true }).page).toBe(1);
    expect(applyPatch(base, { issue: "linked" }).page).toBe(1);
    expect(applyPatch(base, { q: "login" }).page).toBe(1);
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

  it("switching the view resets the page and clears the drawer", () => {
    const jobs = applyPatch(base, { view: "jobs" });
    expect(jobs).toMatchObject({ view: "jobs", page: 1, test: null, job: null, repo: "a/b" });
    const back = applyPatch(applyPatch(base, { view: "jobs", job: 3 }), { view: "tests" });
    expect(back).toMatchObject({ view: "tests", page: 1, test: null, job: null });
  });

  it("test and job are mutually exclusive", () => {
    const withJob = applyPatch({ ...base, test: 7 }, { job: 5 });
    expect(withJob.job).toBe(5);
    expect(withJob.test).toBeNull();
    const withTest = applyPatch({ ...DEFAULT_VIEW, job: 5 }, { test: 9 });
    expect(withTest.test).toBe(9);
    expect(withTest.job).toBeNull();
  });
});
