import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import {
  DEFAULT_VIEW,
  LIST_FILTERS,
  applyPatch,
  parseViewState,
  serializeViewState,
  useViewState,
  type ListFilterDescriptor,
  type ViewState,
} from "./urlState";

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
      pipeline: null,
      newDays: null,
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
      pipeline: null,
      newDays: null,
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
      pipeline: null,
      newDays: null,
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

  it("serializes the ci-run view with no run selected as just the view", () => {
    expect(serializeViewState({ ...DEFAULT_VIEW, view: "ci-run" })).toBe("?view=ci-run");
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

  it("round-trips the new-in-the-last-7-days filter", () => {
    const view = { ...DEFAULT_VIEW, repo: "a/b", newDays: 7 };
    expect(serializeViewState(view)).toBe("?repo=a%2Fb&new=7");
    expect(parseViewState("?repo=a%2Fb&new=7")).toEqual(view);
    expect(serializeViewState({ ...DEFAULT_VIEW, newDays: null })).toBe("");
    // Unscoped by view, like cause and issue: it survives the Jobs view too.
    expect(serializeViewState({ ...DEFAULT_VIEW, view: "jobs", newDays: 7 })).toBe(
      "?view=jobs&new=7",
    );
  });

  it("ignores a non-positive or invalid new filter", () => {
    expect(parseViewState("?new=0").newDays).toBeNull();
    expect(parseViewState("?new=abc").newDays).toBeNull();
    expect(parseViewState("?new=").newDays).toBeNull();
    expect(parseViewState("").newDays).toBeNull();
  });

  it("round-trips the jobs pipeline filter, omitted when unset", () => {
    const view = {
      ...DEFAULT_VIEW,
      view: "jobs" as const,
      pipeline: ".github/workflows/nightly.yml",
    };
    expect(serializeViewState(view)).toBe("?view=jobs&pipeline=.github%2Fworkflows%2Fnightly.yml");
    expect(parseViewState("?view=jobs&pipeline=.github%2Fworkflows%2Fnightly.yml")).toEqual(view);
    expect(serializeViewState({ ...DEFAULT_VIEW, pipeline: null })).toBe("");
    expect(parseViewState("?pipeline=").pipeline).toBeNull();
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

  it("changing repo clears the pipeline, but an explicit pipeline survives", () => {
    const withPipeline = { ...base, pipeline: ".github/workflows/nightly.yml" };
    expect(applyPatch(withPipeline, { repo: "c/d" })).toMatchObject({
      repo: "c/d",
      pipeline: null,
    });
    expect(applyPatch(withPipeline, { repo: "c/d", pipeline: "ci.yml" }).pipeline).toBe("ci.yml");
  });

  it("sort and stable toggle reset page; selecting a test does not", () => {
    expect(applyPatch(base, { sort: "last_seen" }).page).toBe(1);
    expect(applyPatch(base, { showStable: true }).page).toBe(1);
    expect(applyPatch(base, { cause: "timing" }).page).toBe(1);
    expect(applyPatch(base, { quarantined: true }).page).toBe(1);
    expect(applyPatch(base, { issue: "linked" }).page).toBe(1);
    expect(applyPatch(base, { pipeline: ".github/workflows/nightly.yml" }).page).toBe(1);
    expect(applyPatch(base, { newDays: 7 }).page).toBe(1);
    expect(applyPatch({ ...base, newDays: 7 }, { newDays: null }).page).toBe(1);
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

  it("keeps a page that arrives with the view switch, but still clears the drawer", () => {
    const fromTest = applyPatch({ ...base, page: 3 }, { view: "jobs", page: 2 });
    expect(fromTest).toMatchObject({ view: "jobs", page: 2, test: null, job: null });
    const fromJob = applyPatch(
      { ...DEFAULT_VIEW, view: "jobs", page: 3, job: 9 },
      { view: "tests", page: 2 },
    );
    expect(fromJob).toMatchObject({ view: "tests", page: 2, test: null, job: null });
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

/**
 * The descriptor table is the single source of truth: every declared list filter
 * must survive a URL round-trip and reset the page, with no per-filter list to
 * forget a key in.
 */
describe("LIST_FILTERS", () => {
  // Non-default sample values keyed by param (`stable` backs showStable, `new`
  // backs newDays).
  const SAMPLES: Record<string, unknown> = {
    repo: "a/b",
    project: "backend",
    q: "login",
    sort: "last_seen",
    stable: true,
    cause: "timing",
    quarantined: true,
    issue: "linked",
    pipeline: ".github/workflows/nightly.yml",
    new: 7,
  };
  // A repo so the project round-trip survives the project-without-repo rule, and
  // so the `repo` sample ("a/b") genuinely differs from the base.
  const base = { ...DEFAULT_VIEW, repo: "z/z" };

  it.each(LIST_FILTERS)("round-trips $param and resets the page", (d) => {
    const sample = SAMPLES[d.param];
    const view = { ...base, [d.field]: sample } as ViewState;
    expect(parseViewState(serializeViewState(view))[d.field]).toEqual(sample);
    if (d.relists) {
      // page starts at 4, so only the derived rule can bring it back to 1.
      const patched = applyPatch({ ...base, page: 4 }, { [d.field]: sample } as Partial<ViewState>);
      expect(patched.page).toBe(1);
    }
  });

  it("a descriptor added to the table drives parse, serialize and the page reset", () => {
    // If any of the three derived functions still enumerated params by hand,
    // this scratch entry would be ignored and this test would fail.
    const scratch = {
      param: "flag",
      field: "flag", // deliberately not a ViewState field: the table drives the shape
      parse: (raw: string | null) => raw === "1",
      serialize: (p: URLSearchParams, v: boolean) => {
        if (v) p.set("flag", "1");
      },
      relists: true,
    } as unknown as ListFilterDescriptor; // the one place a ViewState-shaped lie is allowed
    LIST_FILTERS.push(scratch);
    try {
      expect((parseViewState("?flag=1") as ViewState & { flag: boolean }).flag).toBe(true);
      expect(serializeViewState({ ...DEFAULT_VIEW, flag: true } as unknown as ViewState)).toBe(
        "?flag=1",
      );
      // page starts at 4, so only the derived rule can bring it back to 1
      expect(
        applyPatch({ ...DEFAULT_VIEW, page: 4 }, { flag: true } as unknown as Partial<ViewState>)
          .page,
      ).toBe(1);
    } finally {
      LIST_FILTERS.pop();
    }
  });
});

/**
 * The hook is the only thing that writes the address bar, so the interesting
 * behaviour is what it refuses to write: a patch that does not change the
 * serialized view must not push a history entry.
 */
describe("useViewState", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    window.history.replaceState(null, "", "/");
  });

  it("pushes only when the URL actually changes", () => {
    window.history.replaceState(null, "", "/");
    const push = vi.spyOn(window.history, "pushState");
    const { result } = renderHook(() => useViewState());

    // An empty patch serializes back to the current (empty) search.
    act(() => result.current[1]({}));
    expect(push).not.toHaveBeenCalled();

    act(() => result.current[1]({ sort: "proven" }));
    expect(push).toHaveBeenCalledTimes(1);
    expect(push).toHaveBeenCalledWith(null, "", "?sort=proven");
  });
});
