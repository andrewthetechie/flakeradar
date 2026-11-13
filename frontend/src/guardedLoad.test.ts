import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { type LatestWins } from "./api";
import { useGuardedLoad, type GuardedLoader } from "./guardedLoad";

/** A loader whose answer the test hands over by hand. */
interface Deferred<TData> {
  promise: Promise<TData | null>;
  resolve: (data: TData | null) => void;
  reject: (error: unknown) => void;
}

function deferred<TData>(): Deferred<TData> {
  let resolve: (data: TData | null) => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<TData | null>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Lets a microtask-scheduled settle finish before the assertion. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

async function flush() {
  await act(settled);
}

/**
 * A loader that answers from `answers` in call order, recording each run's guard
 * and token, and going quiet once a newer request superseded it — the viewData
 * loaders' contract.
 */
function guarded<TData>(answers: (Promise<TData | null> | undefined)[]) {
  const seen: { guard: LatestWins; request: number }[] = [];
  const load: GuardedLoader<TData> = (guard, request) => {
    const answer = answers[seen.length] ?? Promise.resolve(null);
    seen.push({ guard, request });
    return answer.then((data) => (guard.current(request) ? data : null));
  };
  return {
    load,
    calls: () => seen.length,
    guardOf: (index: number) => seen[index].guard,
    requestOf: (index: number) => seen[index].request,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("superseded answers", () => {
  it("a superseded answer never writes state", async () => {
    const stale = deferred<string>();
    const current = deferred<string>();
    const { load, calls } = guarded([stale.promise, current.promise]);

    const { result } = renderHook(() => useGuardedLoad(load));
    // Leave the mount run unanswered and start the refresh: the newest request
    // supersedes it, which is what "a superseded answer" means.
    await act(async () => {
      result.current.refresh();
      await settled();
    });
    expect(calls()).toBe(2); // mount plus the explicit refresh
    expect(result.current.data).toBeNull();

    await act(async () => {
      stale.resolve("stale");
      await stale.promise;
    });
    expect(result.current.data).toBeNull();

    await act(async () => {
      current.resolve("current");
      await current.promise;
    });
    expect(result.current.data).toBe("current");
  });

  it("a superseded failure raises no error", async () => {
    const stale = deferred<string>();
    const current = deferred<string>();
    const { load, calls } = guarded([stale.promise, current.promise]);

    const { result } = renderHook(() => useGuardedLoad(load));
    await act(async () => {
      result.current.refresh();
      await settled();
    });
    expect(calls()).toBe(2);

    // The run the newest request superseded fails: nothing it carries — here an
    // error for a scope the user already left — reaches state.
    await act(async () => {
      stale.reject(new Error("a scope the user already left"));
      await stale.promise.catch(() => {});
    });
    expect(result.current.error).toBeNull();

    await act(async () => {
      current.resolve("kept");
      await current.promise;
    });
    expect(result.current.error).toBeNull();
    expect(result.current.data).toBe("kept");
  });

  it("a loader answering null is dropped, and never overwrites a live value", async () => {
    const answers = [Promise.resolve("first"), Promise.resolve(null)];
    let calls = 0;
    const load: GuardedLoader<string> = async () => {
      const answer = answers[calls] ?? Promise.resolve(null);
      calls += 1;
      return answer;
    };

    const { result, rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) => useGuardedLoad(fn),
      {
        initialProps: { fn: load },
      },
    );
    await flush();
    expect(result.current.data).toBe("first");

    // Same loader identity: only an explicit refresh re-runs it, and the dropped
    // answer must leave the stored data alone.
    await act(async () => {
      await result.current.refresh();
    });
    await rerender({ fn: load });
    expect(result.current.data).toBe("first");
  });
});

describe("cancelling on cleanup", () => {
  it("an effect-lifetime guard cancelled by unmount keeps a late failure silent", async () => {
    const first = deferred<string>();
    const { load, calls, guardOf, requestOf } = guarded<string>([first.promise]);

    const { result, unmount } = renderHook(() => useGuardedLoad(load, { lifetime: "effect" }));
    await act(async () => {
      // The mount run is left unanswered, so the refresh's run supersedes it.
      result.current.refresh();
      await settled();
    });
    expect(calls()).toBe(2);
    const guard = guardOf(0);

    unmount();
    await act(async () => {
      first.reject(new Error("history fetch died"));
      await first.promise.catch(() => {});
    });
    expect(guard.current(requestOf(0))).toBe(false); // cleanup cancelled it
    expect(result.current.error).toBeNull();
    expect(result.current.data).toBeNull();
  });

  it("an effect-lifetime guard cancelled by a re-run keeps a late failure silent", async () => {
    const stale = deferred<string>();
    const { load, calls, guardOf, requestOf } = guarded<string>([stale.promise]);

    const { result, rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) => useGuardedLoad(fn, { lifetime: "effect" }),
      { initialProps: { fn: load } },
    );
    // The mount run is left unanswered, so the re-run is what cancels it.
    await act(async () => {
      await settled();
    });
    expect(calls()).toBe(1);
    const first = guardOf(0);

    // A new load identity re-runs the load, and the previous run's cleanup
    // cancels the guard that run was using. The re-run settles first; only then
    // does the abandoned answer arrive, which is when "cancelled" has to bite.
    let otherCalls = 0;
    const other: GuardedLoader<string> = (guard, request) => {
      otherCalls += 1;
      return Promise.resolve(guard.current(request) ? "new" : null);
    };
    await act(async () => {
      rerender({ fn: other });
      await settled();
    });
    expect(otherCalls).toBe(1); // the re-run is the run that cancelled the old guard
    expect(first.current(requestOf(0))).toBe(false);

    await act(async () => {
      stale.reject(new Error("the drawer closed"));
      await stale.promise.catch(() => {});
      await settled();
    });
    expect(result.current.error).toBeNull();
    expect(result.current.data).toBe("new");
  });

  it("an effect-lifetime re-run keeps the new run's data against a late success", async () => {
    // The drawer-switch race: the old drawer's answer lands after the new
    // drawer's, and only the newest request may write state.
    const stale = deferred<string>();
    const { load, calls, guardOf, requestOf } = guarded<string>([stale.promise]);

    const { result, rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) => useGuardedLoad(fn, { lifetime: "effect" }),
      { initialProps: { fn: load } },
    );
    await act(async () => {
      await settled();
    });
    expect(calls()).toBe(1);
    const first = guardOf(0);

    const other: GuardedLoader<string> = (guard, request) =>
      Promise.resolve(guard.current(request) ? "new drawer" : null);
    await act(async () => {
      rerender({ fn: other });
      await settled();
    });
    expect(first.current(requestOf(0))).toBe(false); // the switch cancelled the old run
    expect(result.current.data).toBe("new drawer");

    await act(async () => {
      stale.resolve("old drawer");
      await stale.promise;
    });
    expect(result.current.data).toBe("new drawer");
    expect(result.current.error).toBeNull();
  });

  it("an effect-lifetime load builds a fresh guard per run", async () => {
    const seen: LatestWins[] = [];
    const load: GuardedLoader<string> = async (guard, request) => {
      seen.push(guard);
      return request > 0 ? "ok" : null;
    };
    const { result } = renderHook(() => useGuardedLoad(load, { lifetime: "effect" }));
    await act(async () => {
      await result.current.refresh();
    });
    expect(seen.length).toBe(2);
    expect(seen[0]).not.toBe(seen[1]);
  });

  it("a component-lifetime guard cancelled by unmount drops a late answer", async () => {
    const late = deferred<string>();
    const onData = vi.fn();
    let guard: LatestWins | null = null;
    const load: GuardedLoader<string> = (g, request) => {
      guard = g;
      return late.promise.then((data) => (g.current(request) ? data : null));
    };
    const { result, unmount } = renderHook(() => useGuardedLoad(load, { onData }));
    await act(async () => {
      await settled();
    });
    const seen = guard as unknown as LatestWins;
    expect(seen.current(1)).toBe(true); // still the newest while it is in flight

    unmount();
    await act(async () => {
      late.resolve("too late");
      await late.promise;
    });
    expect(seen.current(1)).toBe(false); // the unmount cancelled it
    expect(onData).not.toHaveBeenCalled();
    expect(result.current.data).toBeNull();
  });
});

describe("error mapping", () => {
  it("a non-Error rejection lands as String(e) text", async () => {
    const load: GuardedLoader<string> = () => Promise.reject("plain string failure");
    const { result } = renderHook(() => useGuardedLoad(load));
    await flush();
    expect(result.current.error).toBe("plain string failure");
  });

  it("an Error rejection lands as its message", async () => {
    const load: GuardedLoader<string> = () => Promise.reject(new Error("boom"));
    const { result } = renderHook(() => useGuardedLoad(load));
    await flush();
    expect(result.current.error).toBe("boom");
  });

  it("onErrorChange marks the changes, not every write", async () => {
    let message: string | null = "repeated failure";
    const seen: (string | null)[] = [];
    const load: GuardedLoader<string> = async () => {
      if (message !== null) throw new Error(message);
      return "ok";
    };
    const { result } = renderHook(() =>
      useGuardedLoad(load, { onErrorChange: (m) => seen.push(m) }),
    );
    await flush();
    await act(async () => {
      await result.current.refresh(); // the same message again
    });
    expect(result.current.error).toBe("repeated failure");
    expect(seen).toEqual(["repeated failure"]);

    message = null;
    await act(async () => {
      await result.current.refresh(); // a success leaves the error alone by default
    });
    expect(seen).toEqual(["repeated failure"]);
  });
});

describe("error clearing", () => {
  it("clearErrorOnSuccess clears the error on success", async () => {
    let failing = true;
    const load: GuardedLoader<string> = async () => {
      if (failing) {
        failing = false;
        throw new Error("first attempt failed");
      }
      return "recovered";
    };
    const failures: (string | null)[] = [];
    const { result } = renderHook(() =>
      useGuardedLoad(load, {
        clearErrorOnSuccess: true,
        onErrorChange: (message) => failures.push(message),
      }),
    );
    await flush();
    expect(result.current.error).toBe("first attempt failed");
    expect(result.current.data).toBeNull();

    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.error).toBeNull();
    expect(result.current.data).toBe("recovered");
    expect(failures).toEqual(["first attempt failed", null]);
  });

  it("the default does not clear the error on success", async () => {
    let failing = true;
    const load: GuardedLoader<string> = async () => {
      if (failing) {
        failing = false;
        throw new Error("stale banner");
      }
      return "fine";
    };
    const { result } = renderHook(() => useGuardedLoad(load));
    await flush();
    expect(result.current.error).toBe("stale banner");

    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.data).toBe("fine");
    expect(result.current.error).toBe("stale banner");
  });

  it("a failure clears the data only when clearDataOn answers true", async () => {
    // With data stored from a successful run first, a 500 keeps the rows.
    let failing = false;
    const toggled: GuardedLoader<string> = async () => {
      if (failing) throw new Error("500");
      return "kept across a failed refresh";
    };
    const kept = renderHook(() => useGuardedLoad(toggled, { clearDataOn: () => false }));
    await act(async () => {
      // The mount run settles on its own; the refresh is the second call.
      await settled();
      await kept.result.current.refresh();
    });
    failing = true;
    await act(async () => {
      await kept.result.current.refresh();
    });
    expect(kept.result.current.data).toBe("kept across a failed refresh");
    expect(kept.result.current.error).toBe("500");

    // The 404-shaped predicate clears the rows instead.
    let failWith404 = false;
    const clearing: GuardedLoader<string> = async () => {
      if (failWith404) {
        const error = new Error("no such run");
        Object.assign(error, { status: 404 });
        throw error;
      }
      return "rows";
    };
    const cleared = renderHook(() =>
      useGuardedLoad(clearing, { clearDataOn: (e) => (e as { status?: number }).status === 404 }),
    );
    await act(async () => {
      await cleared.result.current.refresh();
    });
    expect(cleared.result.current.data).toBe("rows");
    failWith404 = true;
    await act(async () => {
      await cleared.result.current.refresh();
    });
    expect(cleared.result.current.data).toBeNull();
    expect(cleared.result.current.error).toBe("no such run");
  });
});

describe("enabled", () => {
  it("enabled false holds no data and runs nothing", async () => {
    const load = vi.fn<GuardedLoader<string>>(async () => "should never appear");
    const { result } = renderHook(() => useGuardedLoad(load, { enabled: false }));
    await flush();
    expect(load).not.toHaveBeenCalled();
    expect(result.current.data).toBeNull();
    expect(result.current.error).toBeNull();

    // An imperative refresh is no exception: a disabled view loads nothing.
    await act(async () => {
      await result.current.refresh();
    });
    expect(load).not.toHaveBeenCalled();
    expect(result.current.data).toBeNull();
  });

  it("an in-flight answer is cancelled when the view is disabled", async () => {
    const gate = deferred<string>();
    let guardSeen: LatestWins | null = null;
    const load: GuardedLoader<string> = (guard, request) => {
      guardSeen = guard;
      return gate.promise.then((data) => (guard.current(request) ? data : null));
    };
    const { rerender } = renderHook(
      ({ on }: { on: boolean }) => useGuardedLoad(load, { enabled: on }),
      { initialProps: { on: true } },
    );
    await flush();
    expect(guardSeen).not.toBeNull();
    const guard = guardSeen as unknown as LatestWins;

    await act(async () => {
      rerender({ on: false });
      await Promise.resolve();
    });
    expect(guard.current(1)).toBe(false);

    await act(async () => {
      gate.resolve("late answer");
      await gate.promise;
    });
    expect(guard.current(1)).toBe(false);
  });

  it("a disabled view's data resets, and enabling it runs the load", async () => {
    const load = vi.fn<GuardedLoader<string>>(async () => "here");
    const { result, rerender } = renderHook(
      ({ on }: { on: boolean }) => useGuardedLoad(load, { enabled: on }),
      { initialProps: { on: true } },
    );
    await flush();
    expect(result.current.data).toBe("here");
    expect(load).toHaveBeenCalledTimes(1);

    await act(async () => {
      rerender({ on: false });
      await Promise.resolve();
    });
    expect(result.current.data).toBeNull();

    await act(async () => {
      rerender({ on: true });
      await Promise.resolve();
    });
    expect(load).toHaveBeenCalledTimes(2);
    expect(result.current.data).toBe("here");
  });

  it("a late failure while disabled raises no error", async () => {
    const gate = deferred<string>();
    const load: GuardedLoader<string> = (guard, request) =>
      gate.promise.then((data) => (guard.current(request) ? data : null));
    const { rerender, result } = renderHook(
      ({ on }: { on: boolean }) => useGuardedLoad(load, { enabled: on }),
      { initialProps: { on: true } },
    );
    await flush();
    await act(async () => {
      rerender({ on: false });
      await Promise.resolve();
    });
    await act(async () => {
      gate.reject(new Error("failed after the view closed"));
      await gate.promise.catch(() => {});
    });
    expect(result.current.error).toBeNull();
  });
});

describe("settle callbacks", () => {
  it("onSettled fires on every current settle, success and failure alike", async () => {
    let failing = false;
    const onSettled = vi.fn();
    const onData = vi.fn();
    const onFailure = vi.fn();
    const load: GuardedLoader<string> = async () => {
      if (failing) throw new Error("second attempt failed");
      return "first";
    };
    const { result } = renderHook(() => useGuardedLoad(load, { onSettled, onData, onFailure }));
    await flush();
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(onData).toHaveBeenCalledWith("first");

    failing = true;
    await act(async () => {
      await result.current.refresh();
    });
    expect(onSettled).toHaveBeenCalledTimes(2);
    expect(onFailure).toHaveBeenCalledTimes(1);
    expect(result.current.error).toBe("second attempt failed");
  });

  it("a superseded run fires none of them", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first.promise, second.promise];
    let calls = 0;
    const load: GuardedLoader<string> = (guard, request) => {
      const answer = answers[calls] ?? Promise.resolve(null);
      calls += 1;
      return answer.then((data) => (guard.current(request) ? data : null));
    };
    const onSettled = vi.fn();
    const onData = vi.fn();
    const onFailure = vi.fn();

    const { result } = renderHook(() => useGuardedLoad(load, { onSettled, onData, onFailure }));
    await act(async () => {
      result.current.refresh();
    });

    await act(async () => {
      first.reject(new Error("dropped"));
      await first.promise.catch(() => {});
    });
    expect(onSettled).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();

    await act(async () => {
      second.resolve("current");
      await second.promise;
    });
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(onData).toHaveBeenCalledWith("current");
  });

  it("a run the loader dropped by answering null fires none of them", async () => {
    const load: GuardedLoader<string> = async () => null;
    const onSettled = vi.fn();
    const onData = vi.fn();
    const onFailure = vi.fn();
    const { result } = renderHook(() => useGuardedLoad(load, { onSettled, onData, onFailure }));
    await act(async () => {
      await result.current.refresh();
    });
    expect(onSettled).not.toHaveBeenCalled();
    expect(onData).not.toHaveBeenCalled();
    expect(onFailure).not.toHaveBeenCalled();
    expect(result.current.data).toBeNull();
  });
});

describe("refresh identity", () => {
  it("keeps its identity across rerenders while load is unchanged", async () => {
    const load: GuardedLoader<string> = async () => "ok";
    const { result, rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) => useGuardedLoad(fn),
      { initialProps: { fn: load } },
    );
    const first = result.current.refresh;
    await flush();
    rerender({ fn: load });
    rerender({ fn: load });
    expect(result.current.refresh).toBe(first);
  });

  it("a load identity change re-runs the load", async () => {
    const loadA = vi.fn<GuardedLoader<string>>(async () => "a");
    const loadB = vi.fn<GuardedLoader<string>>(async () => "b");
    const { result, rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) => useGuardedLoad(fn),
      { initialProps: { fn: loadA } },
    );
    await act(async () => {
      await settled();
    });
    expect(loadA).toHaveBeenCalledTimes(1);
    expect(result.current.data).toBe("a");

    const staleRefresh = result.current.refresh;

    await act(async () => {
      rerender({ fn: loadB });
      // The rerender's effect starts the new run, which resolves synchronously.
      await settled();
    });
    expect(loadA).toHaveBeenCalledTimes(1);
    expect(loadB).toHaveBeenCalledTimes(1);
    expect(result.current.data).toBe("b");
    // The other half of the identity rule: `refresh` follows the load, so a
    // caller listing it in a downstream effect's deps re-runs on a scope change.
    expect(result.current.refresh).not.toBe(staleRefresh);
  });

  it("inline options do not restart the load on rerender", async () => {
    const load = vi.fn<GuardedLoader<string>>(async () => "ok");
    const { rerender } = renderHook(
      ({ fn }: { fn: GuardedLoader<string> }) =>
        // A fresh object and fresh callbacks every render, exactly as App wires it.
        useGuardedLoad(fn, { onSettled: () => {}, clearErrorOnSuccess: true }),
      { initialProps: { fn: load } },
    );
    await flush();
    rerender({ fn: load });
    rerender({ fn: load });
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("refresh resolves when its run settles and never rejects", async () => {
    const failing: GuardedLoader<string> = () => Promise.reject(new Error("nope"));
    const { result } = renderHook(() => useGuardedLoad(failing));
    await act(async () => {
      await expect(result.current.refresh()).resolves.toBeUndefined();
    });
    expect(result.current.error).toBe("nope");
  });
});

describe("the loader seam", () => {
  it("hands the guard and the started token to the loader", async () => {
    const seen: { guard: LatestWins; request: number }[] = [];
    const load: GuardedLoader<string> = async (guard, request) => {
      seen.push({ guard, request });
      return guard.current(request) ? "ok" : null;
    };
    const { result } = renderHook(() => useGuardedLoad(load));
    await flush();
    expect(seen).toHaveLength(1);
    expect(seen[0].request).toBe(1);
    expect(typeof seen[0].guard.current).toBe("function");
    expect(result.current.data).toBe("ok");
  });

  it("admits a plain fetch that ignores the guard and the token", async () => {
    const plain = vi.fn(async () => ({ failures: [], total: 0 }));
    const { result } = renderHook(() => useGuardedLoad(plain));
    await flush();
    expect(plain).toHaveBeenCalledOnce();
    expect(result.current.data).toEqual({ failures: [], total: 0 });
  });
});
