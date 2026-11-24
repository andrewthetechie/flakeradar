// One confirmable human decision, so a view declares its mutation instead of
// repeating the pending-dialog dance. Every decision repeats the same steps: on
// an already-decided row the control acts directly; otherwise the entity is held
// as pending until the dialog confirms, the pending entity clears before the
// call settles, the refresh targets run in order after a success, and any
// rejection is mapped onto the caller's one error surface. All of that lives
// here; the caller names the call, the "already decided" predicate and the
// refresh targets, and renders the dialog for the returned pending entity.
import { useCallback, useRef, useState } from "react";
import { type RowKind, type RowReloadRegistry } from "./guardedLoad";

export interface ConfirmedDecisionOptions<TEntity, TChoice> {
  /**
   * The mutation: `decided` is false on the direct path — where the caller
   * adapts the entity to `setQuarantine(id, false)` and friends — and true on
   * the confirm path, where `choice` carries what the dialog collected.
   */
  call: (entity: TEntity, decided: boolean, choice: TChoice | null) => Promise<unknown>;
  /** An already-decided row toggles directly; nothing opens the dialog for it. */
  isDecided: (entity: TEntity) => boolean;
  /** The list refresh, awaited after every successful call. */
  refresh: () => Promise<void>;
  /**
   * The row this decision mutates, so the load layer refetches the displays
   * showing it: after `refresh`, the open load that registered this row reloads.
   * A row no open display shows resolves with nothing done.
   */
  row?: (entity: TEntity) => { kind: RowKind; id: number };
  /**
   * The registry the row's load registers into — the same value the caller gave
   * the guarded loads, which is what connects this decision to those displays.
   */
  registry: RowReloadRegistry;
  /** Extra reloads, run sequentially after `row`'s — the reload tail both that path and a test can drive, successes only. */
  reloads?: (() => Promise<void>)[];
  /** The caller's one error surface; App passes `setError`. */
  onError: (message: string) => void;
}

export interface ConfirmedDecision<TEntity, TChoice> {
  /** The row control's handler: act directly on a decided row, open the dialog otherwise. */
  toggle: (entity: TEntity) => Promise<void>;
  /** The entity waiting in the dialog, or null. */
  pending: TEntity | null;
  /** Close the dialog and record nothing. */
  cancel: () => void;
  /** Clear the dialog, then run the mutation with the choice it collected. */
  confirm: (choice: TChoice) => Promise<void>;
}

export function useConfirmedDecision<TEntity, TChoice>(
  options: ConfirmedDecisionOptions<TEntity, TChoice>,
): ConfirmedDecision<TEntity, TChoice> {
  const [pending, setPendingState] = useState<TEntity | null>(null);
  // `confirm` reads the held entity at call time, so it stays a stable callback
  // the dialog can take without re-binding per render.
  const pendingRef = useRef<TEntity | null>(null);

  const setPending = useCallback((entity: TEntity | null) => {
    pendingRef.current = entity;
    setPendingState(entity);
  }, []);

  // The options carry inline callbacks that change identity every render, so the
  // flow reads them at settle time instead of closing over a stale copy.
  const optionsRef = useRef<ConfirmedDecisionOptions<TEntity, TChoice>>(options);
  optionsRef.current = options;

  // Shared tail of both paths: call, then the refresh targets in order, and any
  // rejection mapped onto the caller's error surface. Never rethrows.
  const run = useCallback(
    async (entity: TEntity, decided: boolean, choice: TChoice | null): Promise<void> => {
      // Read the callbacks now, so an inline caller's callback is the live one.
      const { call, refresh, registry, row, reloads, onError } = optionsRef.current;
      try {
        await call(entity, decided, choice);
        await refresh();
        // The row's reload joins the same sequential tail: a drawer only
        // refetches once the list it sits over is current again.
        if (row != null) {
          const target = row(entity);
          await registry.reload(target.kind, target.id);
        }
        for (const reload of reloads ?? []) await reload();
      } catch (e: unknown) {
        onError(e instanceof Error ? e.message : String(e));
      }
    },
    [],
  );

  const toggle = useCallback(
    async (entity: TEntity): Promise<void> => {
      if (optionsRef.current.isDecided(entity)) {
        await run(entity, false, null);
        return;
      }
      // The mutation is a human decision: ask for it in the dialog first.
      setPending(entity);
    },
    [run, setPending],
  );

  const confirm = useCallback(
    async (choice: TChoice): Promise<void> => {
      const entity = pendingRef.current;
      if (entity === null) return;
      // The dialog closes before the call settles, whatever the answer.
      setPending(null);
      await run(entity, true, choice);
    },
    [run, setPending],
  );

  const cancel = useCallback((): void => {
    setPending(null);
  }, [setPending]);

  return { toggle, pending, cancel, confirm };
}
