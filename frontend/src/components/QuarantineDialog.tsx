import { useState } from "react";
import type { TestCase } from "../api";
import { breakable } from "../breakable";
import { Overlay } from "./Overlay";

/** What the person decided: an optional reason and when to look again (`null` = never). */
export interface QuarantineChoice {
  reason: string | null;
  expiresInDays: number | null;
}

/** Mirrors the backend's `QuarantineIn.expires_in_days` (1 to 365); `null` is "no expiry". */
const EXPIRY_OPTIONS: { label: string; days: number | null }[] = [
  { label: "Never", days: null },
  { label: "7 days", days: 7 },
  { label: "14 days", days: 14 },
  { label: "30 days", days: 30 },
];

const REASON_MAX_LENGTH = 500;

/**
 * Asks why a Test is being quarantined and when to revisit it. Quarantine is a human
 * decision, so nothing happens until Confirm; Cancel and Escape record nothing.
 */
export function QuarantineDialog({
  test,
  onClose,
  onConfirm,
}: {
  test: TestCase;
  onClose: () => void;
  onConfirm: (choice: QuarantineChoice) => void;
}) {
  const [reason, setReason] = useState("");
  const [expiresInDays, setExpiresInDays] = useState<number | null>(null);

  const confirm = () => {
    const trimmed = reason.trim();
    onConfirm({ reason: trimmed == "" ? null : trimmed, expiresInDays });
  };

  return (
    <Overlay variant="modal" label={`Quarantine ${test.name}`} onClose={onClose}>
      <h2 className="mb-1 text-base font-semibold leading-snug [overflow-wrap:anywhere]">
        Quarantine {breakable(test.name)}
      </h2>
      <div className="mb-3 text-xs text-muted [overflow-wrap:anywhere]">
        {breakable(`${test.repo} / ${test.project}`)}
      </div>
      <label className="mb-3 block text-xs text-text-2">
        Reason (optional)
        <textarea
          className="control mt-1 w-full resize-y text-xs"
          rows={3}
          maxLength={REASON_MAX_LENGTH}
          value={reason}
          placeholder="Why is this Test quarantined?"
          onChange={(e) => setReason(e.target.value)}
        />
      </label>
      <fieldset className="mb-4">
        <legend className="mb-1 text-xs text-text-2">Expires</legend>
        <div className="flex flex-wrap gap-x-4 gap-y-2 text-xs text-text-2">
          {EXPIRY_OPTIONS.map((opt) => (
            <label key={opt.label} className="flex cursor-pointer items-center gap-1.5">
              <input
                type="radio"
                name="quarantine-expiry"
                className="size-3.5 accent-(--signal)"
                checked={expiresInDays === opt.days}
                onChange={() => setExpiresInDays(opt.days)}
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
