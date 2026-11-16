import { afterEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { ApiError } from "./api";
import { useConfirmedDecision } from "./confirmedDecision";

/** A call whose answer the test hands over by hand. */
interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Lets a microtask-scheduled settle finish before the assertion. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

/** One dashboard row: `decided` plays the role of `t.quarantined` or `isAcknowledged(...)`. */
interface Row {
  id: number;
  decided: boolean;
}

/** What the dialog collects, playing the role of a QuarantineChoice / AcknowledgeChoice. */
interface Choice {
  reason: string;
  days: number;
}

const row = (id: number, decided: boolean): Row => ({ id, decided });
const choice: Choice = { reason: "flaky on Windows", days: 7 };

/**
 * A spy that records its label on the shared log, so a test can assert the
 * order the hook ran its steps in.
 */
function step<A extends unknown[], R>(label: string, log: string[], fn: (...args: A) => R) {
  return vi.fn((...args: A): R => {
    log.push(label);
    return fn(...args);
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useConfirmedDecision", () => {
  it("a decided row acts immediately, with no pending state", async () => {
    const answer = deferred<unknown>();
    const log: string[] = [];
    const refresh = deferred<void>();
    const reload = step("reload", log, () => refresh.promise.then(() => {}));
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => answer.promise),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => refresh.promise),
        reloads: [reload],
        onError: () => {},
      }),
    );

    await act(async () => {
      void result.current.toggle(row(7, true));
      await settled();
    });
    // The call is in flight — no dialog was opened, nothing else has started.
    expect(result.current.pending).toBeNull();
    expect(log).toEqual(["call"]);

    await act(async () => {
      answer.resolve(undefined);
      await settled();
    });
    // Refresh is awaited, and the extra reload waits for it.
    expect(log).toEqual(["call", "refresh"]);
    expect(reload).not.toHaveBeenCalled();

    await act(async () => {
      refresh.resolve();
      await settled();
    });
    expect(log).toEqual(["call", "refresh", "reload"]);
    expect(result.current.pending).toBeNull();
  });

  it("an undecided row opens the pending dialog, and nothing posts until confirm", async () => {
    const log: string[] = [];
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => Promise.resolve()),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError: () => {},
      }),
    );

    await act(async () => {
      await result.current.toggle(row(3, false));
    });
    expect(result.current.pending).toEqual(row(3, false));
    expect(log).toEqual([]);
  });

  it("confirm clears the pending entity before the call settles, and keeps it cleared on success", async () => {
    const answer = deferred<unknown>();
    const log: string[] = [];
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => answer.promise),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError: () => {},
      }),
    );
    await act(async () => {
      await result.current.toggle(row(7, false));
    });
    expect(result.current.pending).toEqual(row(7, false));

    await act(async () => {
      void result.current.confirm(choice);
      await settled();
    });
    // The dialog is closed already, while the call is still unanswered.
    expect(result.current.pending).toBeNull();
    expect(log).toEqual(["call"]);

    await act(async () => {
      answer.resolve(undefined);
      await settled();
    });
    expect(result.current.pending).toBeNull();
    expect(log).toEqual(["call", "refresh"]);
  });

  it("confirm clears the pending entity even when the call rejects", async () => {
    const answer = deferred<unknown>();
    const log: string[] = [];
    const onError = vi.fn((message: string) => log.push(`error:${message}`));
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => answer.promise),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError,
      }),
    );
    await act(async () => {
      await result.current.toggle(row(7, false));
    });

    await act(async () => {
      void result.current.confirm(choice);
      await settled();
    });
    expect(result.current.pending).toBeNull();

    await act(async () => {
      answer.reject(new Error("backend down"));
      await answer.promise.catch(() => {});
      await settled();
    });
    expect(result.current.pending).toBeNull();
    expect(log).toEqual(["call", "error:backend down"]);
  });

  it("cancel clears the pending entity and records nothing", async () => {
    const log: string[] = [];
    const call = vi.fn(() => Promise.resolve());
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call,
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError: () => {},
      }),
    );
    await act(async () => {
      await result.current.toggle(row(3, false));
    });
    expect(result.current.pending).toEqual(row(3, false));

    act(() => {
      result.current.cancel();
    });
    expect(result.current.pending).toBeNull();
    expect(call).not.toHaveBeenCalled();
    expect(log).toEqual([]);
  });

  it("confirm with nothing pending records nothing", async () => {
    const log: string[] = [];
    const call = vi.fn(() => Promise.resolve());
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call,
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError: () => {},
      }),
    );

    await act(async () => {
      await result.current.confirm(choice);
    });
    expect(call).not.toHaveBeenCalled();
    expect(log).toEqual([]);
  });

  it("a settled confirm awaits the list refresh, then the extra reloads sequentially after it", async () => {
    const log: string[] = [];
    const refreshGate = deferred<void>();
    const firstReloadGate = deferred<void>();
    const secondReloadGate = deferred<void>();
    const firstReload = step("reload 1", log, () => firstReloadGate.promise);
    const secondReload = step("reload 2", log, () => secondReloadGate.promise);
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => Promise.resolve()),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => refreshGate.promise),
        reloads: [firstReload, secondReload],
        onError: () => {},
      }),
    );
    await act(async () => {
      await result.current.toggle(row(7, false));
    });

    await act(async () => {
      void result.current.confirm(choice);
      await settled();
    });
    // The refresh is in flight — awaited, not started in parallel — and the
    // reloads have not begun.
    expect(log).toEqual(["call", "refresh"]);
    expect(firstReload).not.toHaveBeenCalled();

    await act(async () => {
      refreshGate.resolve();
      await settled();
    });
    // The reloads run after the refresh, one after another, not together.
    expect(log).toEqual(["call", "refresh", "reload 1"]);
    expect(secondReload).not.toHaveBeenCalled();

    await act(async () => {
      firstReloadGate.resolve();
      await settled();
    });
    expect(log).toEqual(["call", "refresh", "reload 1", "reload 2"]);
  });

  it("a rejecting call maps an Error message onto the error callback, and runs no refresh", async () => {
    const log: string[] = [];
    const onError = vi.fn((message: string) => log.push(`error:${message}`));
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => Promise.reject(new Error("quarantine 7 -> 500"))),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError,
      }),
    );
    await act(async () => {
      await result.current.toggle(row(7, false));
    });

    await act(async () => {
      await result.current.confirm(choice);
      await settled();
    });
    expect(onError).toHaveBeenCalledWith("quarantine 7 -> 500");
    expect(log).toEqual(["call", "error:quarantine 7 -> 500"]);
  });

  /**
   * The api rejects failed mutations with an ApiError, so the hook's Error arm
   * must keep its message — the "<url> -> <status>" wording the dashboard
   * banner shows.
   */
  it("a rejecting call maps an ApiError's message onto the error callback", async () => {
    const log: string[] = [];
    const onError = vi.fn((message: string) => log.push(`error:${message}`));
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () =>
          Promise.reject(new ApiError("acknowledge tests 5 -> 500", 500)),
        ),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError,
      }),
    );
    await act(async () => {
      await result.current.toggle(row(5, false));
    });

    await act(async () => {
      await result.current.confirm(choice);
      await settled();
    });
    expect(onError).toHaveBeenCalledWith("acknowledge tests 5 -> 500");
    expect(log).toEqual(["call", "error:acknowledge tests 5 -> 500"]);
  });

  it("a non-Error rejection is mapped through String onto the error callback", async () => {
    const log: string[] = [];
    const onError = vi.fn((message: string) => log.push(`error:${message}`));
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => Promise.reject("backend down")),
        isDecided: (t) => t.decided,
        refresh: step("refresh", log, () => Promise.resolve()),
        onError,
      }),
    );
    await act(async () => {
      await result.current.toggle(row(7, false));
    });

    await act(async () => {
      await result.current.confirm(choice);
      await settled();
    });
    expect(onError).toHaveBeenCalledWith("backend down");
    expect(log).toEqual(["call", "error:backend down"]);
  });

  it("a rejecting direct toggle maps onto the error callback too", async () => {
    const log: string[] = [];
    const onError = vi.fn((message: string) => log.push(`error:${message}`));
    const refresh = vi.fn(() => Promise.resolve());
    const { result } = renderHook(() =>
      useConfirmedDecision<Row, Choice>({
        call: step("call", log, () => Promise.reject(new Error("acknowledge 3 -> 503"))),
        isDecided: (t) => t.decided,
        refresh,
        onError,
      }),
    );

    await act(async () => {
      await result.current.toggle(row(3, true));
    });
    expect(onError).toHaveBeenCalledWith("acknowledge 3 -> 503");
    expect(refresh).not.toHaveBeenCalled();
    expect(result.current.pending).toBeNull();
  });
});
