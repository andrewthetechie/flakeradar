import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import {
  DEFAULT_VIEW,
  LIST_FILTERS,
  applyPatch,
  listFilterSlice,
  parseViewState,
  serializeViewState,
  useViewState,
  type ListFilterDescriptor,
  type ViewState,
  type ViewKind,
} from "./urlState";
import type { JobQuery, TestQuery } from "./api";

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
      broken: false,
      acknowledged: false,
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
      broken: false,
      acknowledged: false,
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
      broken: false,
      acknowledged: false,
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

  it("parses and serializes sort=impact", () => {
    expect(parseViewState("?sort=impact").sort).toBe("impact");
    expect(serializeViewState({ ...DEFAULT_VIEW, sort: "impact" })).toBe("?sort=impact");
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

  it("round-trips the acknowledged filter, emitted after broken", () => {
    const view = { ...DEFAULT_VIEW, repo: "a/b", acknowledged: true, broken: true };
    expect(serializeViewState(view)).toBe("?repo=a%2Fb&broken=1&acknowledged=1");
    expect(parseViewState("?acknowledged=1").acknowledged).toBe(true);
    expect(parseViewState("?repo=a%2Fb&broken=1&acknowledged=1")).toEqual(view);
    expect(serializeViewState({ ...DEFAULT_VIEW, acknowledged: false })).toBe("");
    expect(parseViewState("?acknowledged=0").acknowledged).toBe(false);
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
    expect(applyPatch(base, { sort: "impact" }).page).toBe(1);
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

  // A view switch may carry the drawer it should open: the same patch sets the
  // new view and the id, so the switch must not clear what it just set.
  // (openJobFromTest sets view+jobs.job; openTestFromJob sets view+test.)
  it("a view switch that carries a job keeps it and clears test", () => {
    const fromTest = applyPatch({ ...base, test: 5 }, { view: "jobs", job: 3 });
    expect(fromTest).toMatchObject({ view: "jobs", job: 3, test: null, page: 1 });
  });

  it("a view switch that carries a test keeps it and clears job", () => {
    const fromJob = applyPatch(
      { ...DEFAULT_VIEW, view: "jobs", job: 3 },
      { view: "tests", test: 9 },
    );
    expect(fromJob).toMatchObject({ view: "tests", test: 9, job: null });
  });

  it("a view switch without test or job still clears both", () => {
    const switched = applyPatch({ ...DEFAULT_VIEW, view: "jobs", job: 9 }, { view: "tests" });
    expect(switched).toMatchObject({ view: "tests", test: null, job: null });
    const up = applyPatch({ ...base, test: 5 }, { view: "jobs" });
    expect(up).toMatchObject({ view: "jobs", test: null, job: null });
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
    broken: true,
    acknowledged: true,
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

  // Which views offer which filter is fixed by the backend contract: TestsQuery
  // (crates/flakeradar/src/routes/reads.rs:225-254) accepts repo, project, cause,
  // quarantined, broken and the shared filters, JobsQuery (:301-322) accepts repo,
  // pipeline and the shared ones. Keyed by param, like SAMPLES.
  const VIEWS: Record<string, readonly ViewKind[]> = {
    repo: ["tests", "jobs"],
    project: ["tests"],
    q: ["tests", "jobs"],
    sort: ["tests", "jobs"],
    stable: ["tests", "jobs"],
    cause: ["tests"],
    quarantined: ["tests"],
    broken: ["tests"],
    acknowledged: ["tests", "jobs"],
    issue: ["tests", "jobs"],
    pipeline: ["jobs"],
    new: ["tests", "jobs"],
  };

  it.each(LIST_FILTERS)("$param is offered to exactly the views the contract fixes", (d) => {
    expect(d.views).toEqual(VIEWS[d.param]);
  });

  it.each(LIST_FILTERS)(
    "$param has a wire mapping exactly for the endpoints it is offered to",
    (d) => {
      // Repo and project are the exception: they go out through scopeParams (api.ts),
      // so they carry no wire mapping at all.
      if (d.param === "repo" || d.param === "project") {
        expect(d.wire).toBeUndefined();
        return;
      }
      for (const endpoint of ["tests", "jobs"] as const) {
        // Every view that offers the filter sends it, so it needs a mapping — and no
        // view that does not offer it may have one.
        expect(d.wire?.[endpoint] != null).toBe(d.views.includes(endpoint));
      }
    },
  );

  // A base query per endpoint, in the field names the endpoint reads (api.ts).
  const BASE_TEST_QUERY: TestQuery = {
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
    issue: null,
    newWithinDays: null,
    q: "",
  };
  const BASE_JOB_QUERY: JobQuery = {
    repo: "a/b",
    page: 1,
    pageSize: 50,
    sort: "score",
    showStable: false,
    pipeline: null,
    acknowledged: false,
    issue: null,
    newWithinDays: null,
    q: "",
  };
  /** The `name=value` an endpoint mapping emits, or null when it stays out of the
   *  query string — the same decision `params.set` makes in api.ts. */
  const emitted = (
    wire: { param: string; value: (q: never) => string | null } | undefined,
    q: unknown,
  ): string | null => {
    if (!wire) return null;
    const value = wire.value(q as never);
    return value == null ? null : `${wire.param}=${value}`;
  };
  // Per param: a non-default sample patch (in the endpoint's field names), the
  // `name=value` each endpoint must emit for it, and what the default query emits
  // (`sort` is always sent; every other filter stays out when unset).
  const WIRE: Record<
    string,
    {
      patch: Partial<TestQuery & JobQuery>;
      tests: string | null;
      jobs: string | null;
      defaults: string | null;
    }
  > = {
    // scopeParams owns repo and project: no wire mapping at all.
    repo: { patch: {}, tests: null, jobs: null, defaults: null },
    project: { patch: {}, tests: null, jobs: null, defaults: null },
    q: {
      patch: { q: "login" },
      tests: "q=login",
      jobs: "q=login",
      defaults: null,
    },
    sort: {
      patch: { sort: "last_seen" },
      tests: "sort=last_seen",
      jobs: "sort=last_seen",
      defaults: "sort=score",
    },
    stable: {
      patch: { showStable: true },
      tests: "include_stable=true",
      jobs: "include_stable=true",
      defaults: null,
    },
    cause: {
      patch: { category: "timing" },
      tests: "category=timing",
      jobs: null,
      defaults: null,
    },
    quarantined: {
      patch: { quarantined: true },
      tests: "quarantined=true",
      jobs: null,
      defaults: null,
    },
    broken: {
      patch: { broken: true },
      tests: "broken=true",
      jobs: null,
      defaults: null,
    },
    acknowledged: {
      patch: { acknowledged: true },
      tests: "acknowledged=true",
      jobs: "acknowledged=true",
      defaults: null,
    },
    issue: {
      // The one value transform: "linked" is has_issue=true, "none" is false.
      patch: { issue: "linked" },
      tests: "has_issue=true",
      jobs: "has_issue=true",
      defaults: null,
    },
    pipeline: {
      patch: { pipeline: ".github/workflows/nightly.yml" },
      tests: null,
      jobs: "pipeline=.github/workflows/nightly.yml",
      defaults: null,
    },
    new: {
      patch: { newWithinDays: 7 },
      tests: "new_within_days=7",
      jobs: "new_within_days=7",
      defaults: null,
    },
  };

  it.each(LIST_FILTERS)("$param maps to the wire bytes api.ts writes today", (d) => {
    const want = WIRE[d.param];
    const testQuery = { ...BASE_TEST_QUERY, ...want.patch };
    const jobQuery = { ...BASE_JOB_QUERY, ...want.patch };
    expect(emitted(d.wire?.tests, testQuery)).toBe(want.tests);
    expect(emitted(d.wire?.jobs, jobQuery)).toBe(want.jobs);
    // With every filter at its default, only `sort` goes out on either endpoint.
    expect(emitted(d.wire?.tests, BASE_TEST_QUERY)).toBe(want.defaults);
    expect(emitted(d.wire?.jobs, BASE_JOB_QUERY)).toBe(want.defaults);
  });

  // The wire `order` each endpoint emits in — the byte order of the hand-written
  // params.set calls api.ts had before the table, spelled in URL params. The two
  // endpoints take a filter at different positions (tests sends q last, jobs
  // third), which is why the order lives per wire entry rather than in the table.
  const WIRE_ORDER: Record<"tests" | "jobs", string[]> = {
    tests: [
      "sort",
      "stable",
      "cause",
      "quarantined",
      "broken",
      "issue",
      "new",
      "q",
      "acknowledged",
    ],
    jobs: ["sort", "stable", "q", "pipeline", "issue", "new", "acknowledged"],
  };

  it.each(["tests", "jobs"] as const)(
    "the %s wire order is the pre-table api.ts byte order",
    (endpoint) => {
      const mapped = LIST_FILTERS.flatMap((d) => {
        const w = d.wire?.[endpoint];
        return w ? [{ param: d.param, order: w.order }] : [];
      });
      const byOrder = [...mapped].sort((a, b) => a.order - b.order);
      expect(byOrder.map((e) => e.param)).toEqual(WIRE_ORDER[endpoint]);
      // Every wire entry declares its position: a missing order is a type error,
      // a duplicated one would make the emission order depend on sort stability.
      expect(new Set(mapped.map((e) => e.order)).size).toBe(mapped.length);
    },
  );

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
 * The load-facing projection of the table. The rule that matters is identity:
 * a loader keyed on the slice must keep its key when the view changes only in a
 * non-filter field (App.refresh.test.tsx pins that behaviour end to end), so a
 * drawer opening, a page change, or a CI-run navigation answers the same slice.
 */
describe("listFilterSlice", () => {
  const base: ViewState = { ...DEFAULT_VIEW, repo: "a/b", acknowledged: true };

  it("answers the same slice when only the Test drawer differs", () => {
    // test: 7 versus null — the issue's case. Reference *and* value equality.
    const opened = listFilterSlice({ ...base, test: 7 });
    const closed = listFilterSlice(base);
    expect(opened).toBe(closed);
    expect(opened).toEqual(closed);
  });

  it("a changed filter yields a different slice", () => {
    const plain = listFilterSlice(base);
    const quarantined = listFilterSlice({ ...base, quarantined: true });
    expect(quarantined).not.toBe(plain);
    expect(quarantined.quarantined).toBe(true);
  });

  it("keeps the slice across a page, CI-run, or drawer-only change", () => {
    const plain = listFilterSlice(base);
    expect(listFilterSlice({ ...base, page: 3 })).toBe(plain);
    expect(listFilterSlice({ ...base, ci_run: "12345678", attempt: 2 })).toBe(plain);
    expect(listFilterSlice({ ...base, attempt: 2 })).toBe(plain);
    expect(listFilterSlice({ ...base, job: 4 })).toBe(plain);
    expect(listFilterSlice({ ...base, test: 9 })).toBe(plain);
  });

  it("carries view plus exactly the fields the table owns", () => {
    const slice = listFilterSlice(base);
    // Derived from LIST_FILTERS at runtime: nothing here lists the fields, so a
    // table entry shows up in the slice with no further edit.
    expect(Object.keys(slice).sort()).toEqual(["view", ...LIST_FILTERS.map((d) => d.field)].sort());
    expect(slice.view).toBe("tests");
    expect(slice.repo).toBe("a/b");
    expect(slice.acknowledged).toBe(true);
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
