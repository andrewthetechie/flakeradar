// One guarded load, so a view names its loader instead of running the
// newest-request-wins dance. Every caller repeats the same five steps: hold a
// guard (the component's lifetime, or a fresh one per effect), start a token,
// hand both to the loader, drop superseded answers, and map errors only while
// still current. All of that lives here; the caller renders {data, error} and
// calls refresh. Timers stay with the caller, exactly as api.ts documents for
// the guard itself.
import { useCallback, useEffect, useRef, useState } from "react";
import { latestWins, type LatestWins } from "./api";

/**
 * The loader seam: the hook starts the token and hands the guard and the token
 * in, so a viewData loader keeps its `(guard, request)` signature and answers
 * `null` once a newer request superseded it. A plain fetch may ignore both
 * parameters; anything else it resolves (including `null`) is a real answer.
 */
export type GuardedLoader<TData> = (guard: LatestWins, request: number) => Promise<TData | null>;

/** How long the hook's guard lives. */
export type GuardLifetime = "component" | "effect";

export interface GuardedLoadOptions<TData> {
  /**
   * `"component"` (default) holds one `latestWins()` for the hook's lifetime, so
   * a slow answer for an old scope never writes state. `"effect"` builds a fresh
   * guard per load and cancels it on cleanup and on re-run, so an abandoned
   * drawer's in-flight answer is dropped without superseding anything else.
   */
  lifetime?: GuardLifetime;
  /** While false no load runs, in-flight answers are cancelled, and data resets to null. */
  enabled?: boolean;
  /** A current success clears the error. */
  clearErrorOnSuccess?: boolean;
  /** A current failure clears the data when this answers true. */
  clearDataOn?: (error: unknown) => boolean;
  /** Observers, called for a run that settles while current — never for a dropped one. */
  onData?: (data: TData) => void;
  onFailure?: (error: unknown) => void;
  /** Fires once per current settle, success or failure alike. */
  onSettled?: () => void;
  /**
   * Fires on every error write this hook makes — set or clear, including a repeat
   * of the message it wrote last time — so the hook's own state and the caller's
   * copy never part ways. A caller that shares one error surface across several
   * hooks, or writes to it itself, must accept those repeat writes.
   */
  onErrorChange?: (message: string | null) => void;
}

/** What one loader call came back as, before the guard has had its say. */
type Outcome<TData> = { kind: "ok"; value: TData | null } | { kind: "err"; error: unknown };

/** The latest render's option callbacks, read at settle time. */
type Handlers<TData> = Pick<
  GuardedLoadOptions<TData>,
  "clearDataOn" | "onData" | "onFailure" | "onSettled" | "onErrorChange"
> & { clearErrorOnSuccess: boolean };

/**
 * The hook's private three-state settle for one run of the loader:
 *
 * - **Dropped** — the run is no longer current, or the loader answered `null`:
 *   no state write, no callback, no `onSettled`. The run that superseded it
 *   settles in its place, so a caller counting settlements never starves.
 * - **Current success** — store the data, `onData`, clear the error iff
 *   `clearErrorOnSuccess`, then `onSettled`.
 * - **Current failure** — clear the data iff `clearDataOn(e)`, set the error to
 *   the mapped message, `onFailure`, then `onSettled`.
 *
 * `refresh` resolves when its run settles and never rejects. It is rebuilt only
 * when `load`'s identity changes, so a caller listing `refresh` in a downstream
 * effect's deps gets exactly the trigger set it has today, and inline callbacks
 * cannot restart a poll keyed on it.
 */
export function useGuardedLoad<TData>(
  load: GuardedLoader<TData>,
  options: GuardedLoadOptions<TData> = {},
): { data: TData | null; error: string | null; refresh: () => Promise<void> } {
  const [data, setData] = useState<TData | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The options carry inline callbacks that change identity every render, so the
  // run reads them at settle time instead of closing over a stale copy.
  const optionsRef = useRef<GuardedLoadOptions<TData>>(options);
  optionsRef.current = options;

  // The component-lifetime guard, read only when that lifetime is chosen. An
  // effect-lifetime load builds a fresh guard per run instead, so abandoning one
  // load supersedes its own answers and nothing else.
  const componentGuard = useRef<LatestWins>(latestWins());

  // Cancels whichever guard the latest run used, so cleanup and the next run
  // both reach it — an effect-lifetime guard has no other holder.
  const cancelRef = useRef<(() => void) | null>(null);

  // Every error write goes through to the caller, even one that repeats this
  // hook's own last message: another writer may have put something else on the
  // shared surface in between, and only a write that arrives clears it. React
  // bails out on an identical state value, so the hook's own state costs nothing.
  const writeError = useCallback((message: string | null, h: Handlers<TData>) => {
    setError(message);
    h.onErrorChange?.(message);
  }, []);

  const runOnce = useCallback(async (): Promise<void> => {
    const start = optionsRef.current;
    // A disabled view loads nothing, however its run was started.
    if ((start.enabled ?? true) === false) return;
    // The previous run's answers are done with, and it may have had a guard of
    // its own: only this hook can retire it.
    cancelRef.current?.();
    const guard = start.lifetime === "effect" ? latestWins() : componentGuard.current;
    cancelRef.current = () => guard.cancel();
    const request = guard.start();
    // A loader that throws as well as one that rejects lands the same way, so
    // `refresh` has nothing left to reject with.
    const outcome: Outcome<TData> = await (async () => {
      try {
        return { kind: "ok" as const, value: await load(guard, request) };
      } catch (e: unknown) {
        return { kind: "err" as const, error: e };
      }
    })();
    // Dropped: a newer run, a cleanup or a disable owns the state now.
    if (!guard.current(request)) return;
    // Read the callbacks now, so an inline caller's callback is the live one.
    const latest = optionsRef.current;
    const h: Handlers<TData> = {
      clearErrorOnSuccess: latest.clearErrorOnSuccess ?? false,
      clearDataOn: latest.clearDataOn,
      onData: latest.onData,
      onFailure: latest.onFailure,
      onSettled: latest.onSettled,
      onErrorChange: latest.onErrorChange,
    };
    if (outcome.kind === "ok") {
      // Dropped by the loader's own answer: it saw the newer request itself.
      if (outcome.value === null) return;
      setData(outcome.value);
      h.onData?.(outcome.value);
      if (h.clearErrorOnSuccess) writeError(null, h);
    } else {
      if (h.clearDataOn?.(outcome.error)) setData(null);
      writeError(outcome.error instanceof Error ? outcome.error.message : String(outcome.error), h);
      h.onFailure?.(outcome.error);
    }
    h.onSettled?.();
  }, [load, writeError]);

  const refresh = useCallback(async (): Promise<void> => {
    await runOnce();
  }, [runOnce]);

  const enabled = options.enabled ?? true;
  // Mount, a change of the load's identity — that is, of `refresh` — and
  // `enabled` becoming true start a run; nothing else may. Cleanup cancels the
  // guard the latest run used — the shared one for a component-lifetime load,
  // that run's own for an effect-lifetime one — so a load change, a disable or
  // an unmount drops whatever is still in flight. Disabling also empties the
  // data the view no longer shows.
  useEffect(() => {
    if (!enabled) {
      setData(null);
      return;
    }
    void refresh();
    return () => cancelRef.current?.();
  }, [enabled, refresh]);

  return { data, error, refresh };
}
