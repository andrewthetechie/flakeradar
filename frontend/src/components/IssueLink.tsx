import type { ReactNode, MouseEvent } from "react";

/** Link to a Test's auto-filed GitHub issue; renders nothing until one exists. */
export function IssueLink({
  number,
  url,
  closed,
  className,
  title,
  onClick,
  children,
}: {
  number: number | null;
  url: string | null;
  /** The linked issue was closed as not planned (CONTEXT.md). */
  closed?: boolean;
  className?: string;
  title?: string;
  onClick?: (e: MouseEvent) => void;
  children: ReactNode;
}) {
  if (number == null) return null;
  return (
    <a
      href={url ?? undefined}
      onClick={onClick}
      target="_blank"
      rel="noreferrer"
      className={className}
      title={title}
    >
      {children}
      {closed ? " (closed as not planned)" : null}
    </a>
  );
}
