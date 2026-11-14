import { useEffect, type ReactNode } from "react";

/** Class strings of the two placements, exactly as the overlays spelled them before
 *  the shell existed. Drawers anchor right on z-tier 20/21; the centred dialog sits
 *  on 30/31 because it opens on top of an already-open drawer. */
const PLACEMENT = {
  drawer: {
    scrim: "fixed inset-0 z-20 bg-scrim",
    panel:
      "fixed inset-y-0 right-0 z-21 w-full max-w-190 animate-slide-in overflow-y-auto border-l border-line bg-surface px-5 pt-4 pb-8 shadow-2xl",
  },
  modal: {
    scrim: "fixed inset-0 z-30 bg-scrim",
    panel:
      "fixed inset-x-4 top-1/2 z-31 mx-auto w-full max-w-md -translate-y-1/2 rounded-lg border border-line bg-surface p-5 shadow-2xl",
  },
} as const;

/**
 * Chrome shared by every overlay: the window Escape listener, the click-to-close
 * scrim, `role="dialog"` / `aria-modal` / the label, and — for drawers — the
 * breadcrumb row with its × close affordance. Callers pass content only.
 *
 * Handlers never stop propagation, and nothing traps or moves focus: stacked
 * overlays (the quarantine dialog over a Test drawer) all close on one Escape,
 * which is what each component did on its own before the shell.
 */
export function Overlay({
  variant,
  label,
  onClose,
  breadcrumb,
  children,
}: {
  variant: keyof typeof PLACEMENT;
  /** Accessible name of the dialog; Playwright selects the overlays by it. */
  label: string;
  onClose: () => void;
  /** Drawer only: the small line above the content, in the row that holds the ×. */
  breadcrumb?: ReactNode;
  children: ReactNode;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const cls = PLACEMENT[variant];
  // Drawers are `<aside>`s and the dialog a `<div>`; both take the same chrome.
  const Panel = variant === "drawer" ? "aside" : "div";

  return (
    <>
      <div className={cls.scrim} onClick={onClose} aria-hidden />
      <Panel className={cls.panel} role="dialog" aria-modal="true" aria-label={label}>
        {variant === "drawer" && (
          <div className="mb-2 flex items-start justify-between gap-3">
            <div className="text-xs text-link [overflow-wrap:anywhere]">{breadcrumb}</div>
            <button
              type="button"
              className="btn grid size-8 place-items-center p-0 text-lg leading-none"
              onClick={onClose}
              aria-label="Close"
            >
              ×
            </button>
          </div>
        )}
        {children}
      </Panel>
    </>
  );
}
