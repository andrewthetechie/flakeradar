// The shared fixture every Playwright test in this repo uses. It enforces the
// two global rules the task's contract puts on every test in both projects:
//   - any console error or uncaught page error fails the test, and
//   - an `e2e` (real-server) test fails on any /api/* response of 400 or above
//     the test did not list as expected.
// A new test gets both for free by importing `test` from here instead of
// "@playwright/test", so a future test cannot leave the rules out.
import { expect, test as base } from "@playwright/test";

/** One /api response the test asked for, named by path (the query is the
 *  dashboard's own paging and filtering) and status. */
export interface ApiFailure {
  readonly path: string;
  readonly status: number;
}

const label = (path: string, status: number) => `${path} -> ${status}`;

const RESOURCE_FAILURE = /status of (\d{3})/;

export const test = base.extend<{ apiFailures: ApiFailure[]; _watch: void }>({
  /** The failures this test deliberately causes. Two ways to fill it:
   *  `test.use({ apiFailures: [...] })` when a test knows them up front, or a
   *  push during the body, which is what `mockApi` does — a mock that answers
   *  500 or 422 is the test's own declaration, so no ui test repeats it.
   *  Either way the watcher below reads the array after the body has run. */
  apiFailures: async ({}, use) => {
    await use([]);
  },
  // The auto `_watch` fixture runs around every test body and asserts the
  // invariants after it.
  _watch: [
    async ({ page, apiFailures }, use, testInfo) => {
      const consoleErrors: string[] = [];
      const unexpected: string[] = [];
      const allowed = () => new Set(apiFailures.map((f) => label(f.path, f.status)));

      page.on("console", (msg) => {
        if (msg.type() !== "error") return;
        // Chromium logs a failed response as a console error naming the
        // resource it failed on. When the test mocked that response itself, the
        // error is the state under test (an error banner, a refused write).
        const status = Number(RESOURCE_FAILURE.exec(msg.text())?.[1]);
        const url = msg.location().url;
        if (status && url) {
          const path = new URL(url).pathname;
          if (/^\/api\//.test(path) && allowed().has(label(path, status))) return;
        }
        consoleErrors.push(msg.text());
      });
      page.on("pageerror", (err) => consoleErrors.push(`pageerror: ${err.message}`));
      if (testInfo.project.name === "e2e") {
        page.on("response", (r) => {
          const path = new URL(r.url()).pathname;
          if (!/^\/api\//.test(path) || r.status() < 400) return;
          if (!allowed().has(label(path, r.status()))) unexpected.push(label(path, r.status()));
        });
      }

      await use();
      expect(consoleErrors, `console/page errors:\n${consoleErrors.join("\n")}`).toEqual([]);
      expect(unexpected, `unexpected /api response:\n${unexpected.join("\n")}`).toEqual([]);
    },
    { auto: true },
  ],
});
