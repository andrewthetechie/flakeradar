/** Badge marking a summary Job (alls-green): it runs no tests, so its explained
 * failures come from a failed sibling job in the same CI run attempt. */
export function SummaryBadge({ className = "" }: { className?: string }) {
  return (
    <span
      title="Runs no tests; explained by a failed job in the same CI run attempt"
      className={`rounded-full border border-line px-2 py-0.5 align-middle text-xs font-normal text-muted ${className}`}
    >
      summary
    </span>
  );
}
