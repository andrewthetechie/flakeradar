import { Fragment, type ReactNode } from "react";

/** Long test ids (`tests.test_cron_health.TestSuccessfulTickStamps`,
 *  `writers-app::summarize`, `src/a/b.test.ts`) have no spaces, so browsers
 *  either overflow or break mid-word. Offer a line break after each
 *  separator instead: `.`, `:`, `/`, `_`, `-`. */
export function breakable(text: string): ReactNode {
  const parts = text.split(/(?<=[.:/_-])/);
  return parts.map((part, i) => (
    <Fragment key={i}>
      {part}
      {i < parts.length - 1 && <wbr />}
    </Fragment>
  ));
}
