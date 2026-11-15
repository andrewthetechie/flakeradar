import { useState } from "react";
import { breakable } from "../breakable";
import { Overlay } from "./Overlay";

/** What the person decided: an optional reason and how long the acknowledgement lasts. */
export interface AcknowledgeChoice {
  reason: string | null;
  days: 1 | 7 | 30;
}

/** Mirrors the backend's `AcknowledgeIn.days` (1 to 30); the UI only offers these three. */
const DAY_OPTIONS: { label: string; days: 1 | 7 | 30 }[] = [
  { label: "1 day", days: 1 },
  { label: "7 days", days: 7 },
  { label: "30 days", days: 30 },
];

const REASON_MAX_LENGTH = 500;

/**
 * Asks why something is acknowledged and for how long. Built like `QuarantineDialog` but
 * taking a name and a scope line as plain strings, so Tests and Jobs share one dialog.
 * Acknowledge is a human decision, so nothing happens until Confirm; Cancel, Escape and
 * the scrim record nothing.
 */
export function AcknowledgeDialog({
  name,
  scope,
  onClose,
  onConfirm,
}: {
  name: string;
  scope: string;
  onClose: () => void;
  onConfirm: (choice: AcknowledgeChoice) => void;
}) {
  const [reason, setReason] = useState("");
  const [days, setDays] = useState<1 | 7 | 30>(7);

  const confirm = () => {
    const trimmed = reason.trim();
    onConfirm({ reason: trimmed == "" ? null : trimmed, days });
  };

  return (
    <Overlay variant="modal" label={`Acknowledge ${name}`} onClose={onClose}>
      <h2 className="mb-1 text-base font-semibold leading-snug [overflow-wrap:anywhere]">
        Acknowledge {breakable(name)}
      </h2>
      <div className="mb-3 text-xs text-muted [overflow-wrap:anywhere]">{breakable(scope)}</div>
      <label className="mb-3 block text-xs text-text-2">
        Reason (optional)
        <textarea
          className="control mt-1 w-full resize-y text-xs"
          rows={3}
          maxLength={REASON_MAX_LENGTH}
          value={reason}
          placeholder="Why is this acknowledged?"
          onChange={(e) => setReason(e.target.value)}
        />
      </label>
      <fieldset className="mb-4">
        <legend className="mb-1 text-xs text-text-2">Acknowledged for</legend>
        <div className="flex flex-wrap gap-x-4 gap-y-2 text-xs text-text-2">
          {DAY_OPTIONS.map((opt) => (
            <label key={opt.label} className="flex cursor-pointer items-center gap-1.5">
              <input
                type="radio"
                name="acknowledge-days"
                className="size-3.5 accent-(--signal)"
                checked={days === opt.days}
                onChange={() => setDays(opt.days)}
              />
              {opt.label}
            </label>
          ))}
        </div>
      </fieldset>
      <div className="flex justify-end gap-2">
        <button type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="btn" onClick={confirm}>
          Confirm
        </button>
      </div>
    </Overlay>
  );
}
