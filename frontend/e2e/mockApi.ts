// Route table for the `ui` (mocked) Playwright project. One catch-all
// `**/api/**` route answers every call from `frontend/src`; a call the test did
// not mock is collected in `assertNoUnmocked()`, which fails the test. Matching
// is on the request path, so one entry covers every query the dashboard sends.
import { expect, type Page } from "@playwright/test";
import type { ApiFailure } from "./sharedFixture";

export interface Mock {
  /** Match against the request's path (no query string), as a RegExp. */
  match: RegExp;
  /** HTTP status to answer; default 200. */
  status?: number;
  /** JSON body to fulfill. */
  body?: unknown;
  /** Per-request body, for routes that vary by query (pagination). */
  bodyFn?: (url: URL) => unknown;
  /** Called each time this entry answers, so a test can count requests. */
  onCall?: () => void;
}

export interface InstalledMocks {
  /** Fail the test if any /api/* request was not mocked. Call at the end. */
  assertNoUnmocked: () => void;
}

/**
 * Install the mock routes for one ui test.
 *
 * `declared` is the test's `apiFailures` fixture: every answer of 400 or above
 * is recorded there, because the browser also logs it as a console error and
 * the shared fixture would otherwise fail the test for the very state it
 * mocks. An unmocked path is answered 500 and deliberately NOT recorded, so it
 * fails on both rules as well as in `assertNoUnmocked`.
 */
export function mockApi(page: Page, declared: ApiFailure[], mocks: Mock[]): InstalledMocks {
  const unmocked: string[] = [];
  void page.route("**/api/**", (route) => {
    const url = new URL(route.request().url());
    const mock = mocks.find((m) => m.match.test(url.pathname));
    if (!mock) {
      unmocked.push(`${route.request().method()} ${url.pathname}`);
      void route.fulfill({ status: 500, contentType: "application/json", body: "{}" });
      return;
    }
    const status = mock.status ?? 200;
    if (status >= 400) declared.push({ path: url.pathname, status });
    mock.onCall?.();
    const body = mock.bodyFn ? mock.bodyFn(url) : mock.body;
    void route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
  return {
    assertNoUnmocked: () =>
      expect(unmocked, `unmocked /api calls:\n${unmocked.join("\n")}`).toEqual([]),
  };
}
