import type { ReactNode, MouseEvent } from "react";

/** Link to a Test's auto-filed GitHub issue; renders nothing until one exists. */
export function IssueLink({
  number,
  url,
  className,
  title,
  onClick,
  children,
}: {
  number: number | null;
  url: string | null;
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
    </a>
  );
}
