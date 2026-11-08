// The `ui` project: the built dashboard served statically with every /api/*
// call answered by page.route from fixtures in e2e/fixtures/. It exercises the
// states a real server rarely produces — empty lists, a failing read, a refused
// write, a pending queue, a long name, a null category and trend, and the
// pagination boundary.
import { expect } from "@playwright/test";
import { mockApi } from "./mockApi";
import { readFixture } from "./fixtures";
import { test } from "./sharedFixture";

const emptyTestsPage = { items: [], total: 0, page: 1, page_size: 50 };
const emptyJobsPage = { items: [], total: 0, page: 1, page_size: 50 };
const zeroSummary = {
  total_tests: 0,
  flaky_tests: 0,
  suspect_tests: 0,
  confirmed_flaky_tests: 0,
  new_flaky_7d: 0,
  total_runs: 0,
  executions_30d: 0,
  failures_30d: 0,
  flake_threshold: 0.5,
  category_counts: { network: 0, environment: 0, timing: 0, assertion: 0, other: 0 },
};
const zeroJobSummary = {
  total_jobs: 0,
  flaky_jobs: 0,
  suspect_jobs: 0,
  confirmed_flaky_jobs: 0,
  new_flaky_7d: 0,
  job_executions_30d: 0,
  job_failures_30d: 0,
  flake_threshold: 0.5,
  pipelines: [],
};

test.describe("ui: mocked API states", () => {
  test("empty lists: no repos, no tests, no jobs render their empty states", async ({
    page,
    apiFailures,
  }) => {
    const mocks = mockApi(page, apiFailures, [
      { match: /^\/api\/repos$/, body: [] },
      { match: /^\/api\/summary$/, body: zeroSummary },
      { match: /^\/api\/tests$/, body: emptyTestsPage },
      { match: /^\/api\/reports\/summary$/, body: { pending: 0, failed: 0 } },
      { match: /^\/api\/jobs\/summary$/, body: zeroJobSummary },
      { match: /^\/api\/jobs$/, body: emptyJobsPage },
    ]);
    await page.goto("/");
    await expect(page.getByText("No flaky or suspect tests in this view.")).toBeVisible();
    // The scope picker has the "All repos" option and nothing else, and the
    // Project picker stays disabled while no Repo is chosen.
    const repoPicker = page.getByLabel("Repo");
    await expect(repoPicker.locator("option")).toHaveCount(1);
    await expect(repoPicker).toHaveValue("");
    await expect(page.getByLabel("Project")).toBeDisabled();
    // The tiles say zero, and the queue indicator is absent on an empty queue.
    await expect(page.getByTestId("stat-tile")).toHaveCount(8);
    await expect(page.getByTestId("stat-tile").first()).toContainText("0");
    await expect(page.getByRole("button", { name: /pending · \d+ failed/ })).toHaveCount(0);

    await page.goto("/?view=jobs");
    await expect(page.getByText("No flaky or suspect jobs in this view.")).toBeVisible();
    await expect(page.getByTestId("stat-tile")).toHaveCount(7);
    mocks.assertNoUnmocked();
  });

  test("a 500 on every read shows the error banner", async ({ page, apiFailures }) => {
    const mocks = mockApi(page, apiFailures, [
      { match: /^\/api\/repos$/, status: 500 },
      { match: /^\/api\/summary$/, status: 500 },
      { match: /^\/api\/tests$/, status: 500 },
      { match: /^\/api\/reports\/summary$/, status: 500 },
    ]);
    await page.goto("/");
    const banner = page.getByRole("alert");
    await expect(banner).toContainText("Could not reach the FlakeRadar API");
    // One of the reads the view makes is named, with its status, as `api.ts`
    // words it. Which one reaches the banner first is the loader's own race.
    await expect(banner).toContainText(/\/api\/(repos|summary|tests|reports\/summary) -> 500/);
    // Nothing was rendered as if it were data.
    await expect(page.getByTestId("stat-tile")).toHaveCount(0);
    mocks.assertNoUnmocked();
  });

  test("a 422 on a quarantine write shows the error banner and sends no second change", async ({
    page,
    apiFailures,
  }) => {
    let quarantineCalls = 0;
    const mocks = mockApi(page, apiFailures, [
      { match: /^\/api\/repos$/, body: readFixture("repos") },
      { match: /^\/api\/summary$/, body: readFixture("summary") },
      { match: /^\/api\/tests$/, body: readFixture("tests") },
      { match: /^\/api\/reports\/summary$/, body: readFixture("reportsSummary") },
      {
        match: /^\/api\/tests\/\d+\/quarantine$/,
        status: 422,
        body: { detail: "expires_in_days beyond 365" },
        onCall: () => {
          quarantineCalls++;
        },
      },
    ]);
    await page.goto("/?repo=demo/shop");
    const row = page.getByRole("row").filter({ hasText: "test_payment_gateway_timeout" });
    await row.getByRole("button", { name: "Quarantine" }).click();
    // The dialog asks for the reason and the expiry before anything is sent.
    const dialog = page.getByRole("dialog", { name: /Quarantine test_payment_gateway_timeout/ });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel(/Reason/)).toBeVisible();
    await expect(dialog.getByRole("radio", { name: "30 days" })).toBeVisible();
    expect(quarantineCalls).toBe(0);
    await dialog.getByRole("button", { name: "Confirm" }).click();
    await expect(page.getByRole("alert")).toContainText("quarantine 2 -> 422");
    // One write only: a refused change is not retried.
    await expect.poll(() => quarantineCalls).toBe(1);
    mocks.assertNoUnmocked();
  });

  test("a pending queue shows the indicator and lists the failed reports", async ({
    page,
    apiFailures,
  }) => {
    const mocks = mockApi(page, apiFailures, [
      { match: /^\/api\/repos$/, body: readFixture("repos") },
      { match: /^\/api\/summary$/, body: readFixture("summary") },
      { match: /^\/api\/tests$/, body: readFixture("tests") },
      { match: /^\/api\/reports\/summary$/, body: readFixture("reportsSummaryPending") },
      { match: /^\/api\/reports$/, body: readFixture("reportsFailed") },
    ]);
    await page.goto("/");
    const indicator = page.getByRole("button", { name: "3 pending · 1 failed" });
    await expect(indicator).toBeVisible();
    await indicator.click();
    const panel = page.getByRole("region", { name: "Failed reports" });
    await expect(panel.getByText("parse failed: unexpected token")).toBeVisible();
    await expect(panel.getByText("#42 · demo/shop / backend · abcdef1234")).toBeVisible();
    mocks.assertNoUnmocked();
  });

  test("a long name, a null category and a null trend render, and page 2 reaches the boundary", async ({
    page,
    apiFailures,
  }) => {
    const longName =
      "test_checkout_total_rounding_with_a_payment_gateway_that_clamps_to_two_decimals_and_then_rounds_bankers_half_even_on_a_decimal_edge_case_so_the_total_stays_at_104_85";
    const mocks = mockApi(page, apiFailures, [
      { match: /^\/api\/repos$/, body: readFixture("repos") },
      { match: /^\/api\/summary$/, body: readFixture("summary") },
      {
        match: /^\/api\/tests$/,
        bodyFn: (url) =>
          url.searchParams.get("page") === "2" ? readFixture("testsPage2") : readFixture("tests"),
      },
      { match: /^\/api\/reports\/summary$/, body: readFixture("reportsSummary") },
    ]);
    await page.goto("/?repo=demo/shop");
    // The long, separator-free name is present and wrapped rather than clipped.
    const longRow = page.getByRole("row").filter({ hasText: longName });
    await expect(longRow).toBeVisible();
    // Its null `failure_category` shows no likely-cause badge and its null
    // `trend` shows no trend mark; the tier label still shows.
    await expect(longRow).not.toContainText("likely:");
    await expect(longRow.getByTitle(/versus 14 days ago/)).toHaveCount(0);
    await expect(longRow).toContainText("flaky");
    // A row that does carry both shows them, so the two assertions above are not
    // vacuous.
    const timedRow = page.getByRole("row").filter({ hasText: "test_payment_gateway_timeout" });
    await expect(timedRow).toContainText("likely: timing");
    await expect(timedRow.getByTitle(/Worsening/)).toHaveCount(1);

    // Pagination boundary: 60 Tests at 50 per page is two pages.
    const pages = page.getByRole("navigation", { name: "Leaderboard pages" });
    await expect(pages).toContainText("Page 1 of 2 · 1–50 of 60");
    await expect(pages.getByRole("button", { name: "‹ Prev" })).toBeDisabled();
    await pages.getByRole("button", { name: "Next ›" }).click();
    await expect(
      page.getByRole("row").filter({ hasText: "test_a_brand_new_flaky_case_on_the_second_page" }),
    ).toBeVisible();
    await expect(pages).toContainText("Page 2 of 2 · 51–60 of 60");
    await expect(pages.getByRole("button", { name: "Next ›" })).toBeDisabled();
    mocks.assertNoUnmocked();
  });
});
