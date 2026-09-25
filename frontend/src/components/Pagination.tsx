export function Pagination({
  page,
  pageSize,
  total,
  onPage,
}: {
  page: number;
  pageSize: number;
  total: number;
  onPage: (page: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const first = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const last = Math.min(page * pageSize, total);
  return (
    <nav
      className="flex items-center justify-between gap-3 border-t border-line px-4 py-3 text-xs text-text-2"
      aria-label="Leaderboard pages"
    >
      <button type="button" className="btn" onClick={() => onPage(page - 1)} disabled={page <= 1}>
        ‹ Prev
      </button>
      <span className="tabular-nums">
        Page {page} of {pages} · {first}–{last} of {total}
      </span>
      <button
        type="button"
        className="btn"
        onClick={() => onPage(page + 1)}
        disabled={page >= pages}
      >
        Next ›
      </button>
    </nav>
  );
}
