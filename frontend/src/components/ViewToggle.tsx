import type { ViewKind } from "../urlState";

/** Segmented "Tests / Jobs" control for the dashboard header. */
export function ViewToggle({
  view,
  onChange,
}: {
  view: ViewKind;
  onChange: (v: ViewKind) => void;
}) {
  return (
    <div
      role="group"
      aria-label="View"
      className="flex overflow-hidden rounded-lg border border-line bg-surface"
    >
      {(["tests", "jobs"] as const).map((v) => (
        <button
          key={v}
          type="button"
          aria-pressed={view === v}
          onClick={() => onChange(v)}
          className={`px-3 py-1.5 text-xs font-medium uppercase tracking-wide ${
            view === v ? "bg-surface-2 text-text shadow-inner" : "text-muted hover:text-text-2"
          }`}
        >
          {v}
        </button>
      ))}
    </div>
  );
}
