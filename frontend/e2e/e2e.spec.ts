// The `e2e` project: a browser against a real install. Playwright starts
// `e2e/scripts/start-mock-install.mjs`, which boots postgres:17-alpine, runs the
// `flakeradar` binary with `frontend/dist` embedded, and seeds it with
// `samples/simulate_ci.py`. Every number below is that seed's answer: the seeder
// draws from `random.Random(42)`, so the same 14 CI runs of demo/shop are
// replayed on every machine and the scores, streaks and counts are fixed.
import { expect, type Locator, type Page } from "@playwright/test";
import { answerFields, fixtureFields } from "./fixtures";
import { test } from "./sharedFixture";

const REPO = "demo/shop";
const DASH = `/?repo=${REPO}`;

/** The Tests leaderboard's data rows: each has the Quarantine button, the header row does not. */
const testRows = (page: Page): Locator =>
  page.getByRole("row").filter({ has: page.getByRole("button", { name: /quarantine/i }) });

const rowNames = async (rows: Locator): Promise<string[]> =>
  (await rows.allTextContents()).map((text) => text.replace(/\s+/g, " ").trim());

/**
 * The leaderboard rows of a freshly loaded page, once they arrive. Waiting on
 * the count first is what makes the comparison deterministic: right after a
 * navigation the table is empty while the reads are in flight.
 */
const expectRows = async (page: Page, expected: string[]) => {
  const rows = testRows(page);
  await expect(rows).toHaveCount(expected.length);
  expect(await rowNames(rows)).toEqual(expected);
};

/** One stat tile, by the label it shows above its value. */
const tile = (page: Page, label: string): Locator =>
  page.getByTestId("stat-tile").filter({ hasText: label });

/** One drawer stat block: the value is the sibling under its label. */
const statValue = (drawer: Locator, label: string): Locator =>
  drawer.getByText(label, { exact: true }).locator("xpath=../div[2]");

test.describe("e2e: the shipped dashboard on a mock install", () => {
  test("the tiles show the seeded counts and the leaderboard lists worst first", async ({
    page,
  }) => {
    await page.goto(DASH);
    await expect(tile(page, "Tracked tests")).toContainText("10");
    await expect(tile(page, "Flaky (score")).toContainText("3");
    await expect(tile(page, "Suspect (score > 0)")).toContainText("1");
    await expect(tile(page, "Proven flaky")).toContainText("2");
    await expect(tile(page, "New in 7 days")).toContainText("3");
    await expect(tile(page, "CI runs ingested")).toContainText("29");
    await expect(tile(page, "Executions (30 days)")).toContainText("152");
    await expect(tile(page, "Failures (30 days)")).toContainText("36");

    // The default list is the flaky and suspect Tests only, worst first, each
    // with its tier and its likely cause.
    const rows = testRows(page);
    await expect(rows).toHaveCount(4);
    const listed: [string, string, string][] = [
      ["Checkout > shows the total", "0.90", "flaky"],
      ["Cart > updates the badge", "0.62", "flaky"],
      ["test_checkout_total_rounding", "0.60", "flaky"],
      ["test_payment_gateway_timeout", "0.02", "suspect"],
    ];
    for (const [i, [name, score, tier]] of listed.entries()) {
      await expect(rows.nth(i)).toContainText(name);
      await expect(rows.nth(i)).toContainText(score);
      await expect(rows.nth(i)).toContainText(tier);
    }
    // The likely cause comes from the failure messages of the seeded failures.
    await expect(rows.nth(0)).toContainText("likely: timing");
    await expect(rows.nth(1)).toContainText("likely: assertion");
    // Proof: the seed's same-SHA retry puts four Proven flakes on the worst row.
    await expect(rows.nth(0)).toContainText("4× proven");
    // Worst first, read back off the page rather than trusted from the seed.
    const scores = await Promise.all(
      listed.map(async (_, i) =>
        Number((await rows.nth(i).textContent())?.match(/(\d\.\d\d)/)?.[1] ?? NaN),
      ),
    );
    expect(scores).toEqual([...scores].sort((a, b) => b - a));
    // A Test broken on the Default branch scores 0 and is not listed as flaky.
    await expect(rows.filter({ hasText: "test_schema_migration_v42" })).toHaveCount(0);
  });

  test("clicking a Test row opens its detail with history and the latest failure", async ({
    page,
  }) => {
    await page.goto(DASH);
    await page.getByRole("row").filter({ hasText: "test_checkout_total_rounding" }).click();
    await expect(page).toHaveURL(/test=\d+/);

    const drawer = page.getByRole("dialog", { name: "Test detail" });
    await expect(drawer.getByRole("heading", { level: 2 })).toContainText(
      "test_checkout_total_rounding",
    );
    await expect(drawer).toContainText(`${REPO} / backend`);
    await expect(statValue(drawer, "Flakiness score")).toHaveText("0.60");
    await expect(statValue(drawer, "Proven flakes")).toHaveText("1");
    await expect(statValue(drawer, "Clean streak")).toHaveText("3");
    // The Location the latest report gave, permalinked at the failing commit.
    await expect(drawer.getByRole("link", { name: "tests/e2e/test_shop.py:88" })).toHaveAttribute(
      "href",
      /github\.com\/demo\/shop\/blob\/[0-9a-f]{40}\/tests\/e2e\/test_shop\.py#L88/,
    );
    // Every Execution: the strip is oldest to newest, the table newest first.
    await expect(
      drawer.getByRole("img", { name: /^Execution history, oldest to newest:/ }),
    ).toBeVisible();
    await expect(drawer.getByRole("table").last().getByRole("row")).toHaveCount(16); // + header
    // The latest failure, with its message and its traceback.
    await expect(drawer.getByText(/^Latest failure · /)).toBeVisible();
    await expect(drawer.locator("pre")).toContainText(
      "AssertionError: expected 104.85, got 104.84",
    );
    // The Job that ran it: how the Tests view reaches the Jobs view.
    await expect(
      drawer.getByRole("button", { name: ".github/workflows/ci.yml · backend" }),
    ).toBeVisible();
  });

  test("the Jobs view lists the seeded Jobs and a row opens its executions", async ({ page }) => {
    await page.goto(`${DASH}&view=jobs`);
    await expect(tile(page, "Tracked jobs")).toContainText("4");
    await expect(tile(page, "Flaky (score")).toContainText("1");
    await expect(tile(page, "Job executions (30 days)")).toContainText("58");
    await expect(tile(page, "Job failures (30 days)")).toContainText("18");

    const row = page.getByRole("row").filter({ hasText: "e2e (ubuntu-latest)" });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText("0.36");
    await expect(row).toContainText("flaky");
    // No Quarantine column: only a Test is ever quarantined.
    await expect(row.getByRole("button")).toHaveCount(0);

    await row.click();
    await expect(page).toHaveURL(/job=\d+/);
    const drawer = page.getByRole("dialog", { name: "Job detail" });
    await expect(drawer.getByRole("heading", { level: 2 })).toContainText("e2e (ubuntu-latest)");
    // Unexplained failures are the ones that count against the Job's score.
    await expect(drawer).toContainText(/3 unexplained failures/);
    await expect(drawer).toContainText(/0 explained by tests/);
    await expect(drawer).toContainText(/last 15 executions/);
    await expect(drawer).toContainText(/clean streak 0/);
    // The runner table, worst unexplained rate first.
    const runners = drawer.getByRole("table", { name: "Failures by runner" });
    await expect(runners.getByRole("row")).toHaveCount(3); // + header
    await expect(runners.getByRole("row").nth(1)).toContainText("ubuntu-latest");
    await expect(runners.getByRole("row").nth(2)).toContainText("self-hosted, ubuntu-latest");
    // The Job's own executions, each with its CI run attempt.
    await expect(drawer.getByRole("button", { name: /^run run-\d+$/ }).first()).toBeVisible();
    await expect(drawer.getByText("attempt 1").first()).toBeVisible();
  });

  test("a CI run shows its failing Tests and its failed Jobs, explained or unexplained", async ({
    page,
  }) => {
    await page.goto(`${DASH}&view=ci-run&ci_run=run-12`);
    await expect(page.getByRole("heading", { name: "CI run run-12" })).toBeVisible();
    await expect(page.getByText("3 reports processed")).toBeVisible();

    const tables = page.getByRole("table");
    const failedTests = tables.nth(0).getByRole("row");
    await expect(failedTests).toHaveCount(4); // + header
    await expect(failedTests.filter({ hasText: "test_schema_migration_v42" })).toContainText(
      "failed",
    );
    await expect(failedTests.filter({ hasText: "Cart > updates the badge" })).toContainText(
      "likely: assertion",
    );
    // A Test that failed and then passed in the same Run is a retry, not a fix.
    await expect(failedTests.filter({ hasText: "Checkout > shows the total" })).toContainText(
      "passed on retry",
    );

    await expect(page.getByRole("heading", { name: "Failed jobs" })).toBeVisible();
    const failedJobs = tables.nth(1).getByRole("row");
    await expect(failedJobs).toHaveCount(3); // + header
    // The infrastructure flake has no failing Test behind it; the frontend
    // failure does, and each row says which.
    await expect(failedJobs.filter({ hasText: "e2e (ubuntu-latest)" })).toContainText(
      "unexplained",
    );
    await expect(failedJobs.filter({ hasText: "frontend" })).toContainText("explained");
  });

  test("quarantining asks for a reason and an expiry, and the API agrees", async ({ page }) => {
    await page.goto(DASH);
    const name = "test_payment_gateway_timeout";
    await page
      .getByRole("row")
      .filter({ hasText: name })
      .getByRole("button", { name: "Quarantine" })
      .click();

    // Nothing is sent until Confirm: Quarantine is a human decision.
    const dialog = page.getByRole("dialog", { name: `Quarantine ${name}` });
    await expect(dialog.getByText(`${REPO} / backend`)).toBeVisible();
    await dialog.getByLabel("Reason (optional)").fill("network: the payment gateway times out");
    await dialog.getByRole("radio", { name: "30 days" }).check();
    const posted = page.waitForResponse(
      (r) =>
        r.request().method() === "POST" &&
        /^\/api\/tests\/\d+\/quarantine$/.test(new URL(r.url()).pathname),
    );
    await dialog.getByRole("button", { name: "Confirm" }).click();
    const response = await posted;
    expect(response.status()).toBe(200);
    const written = (await response.json()) as {
      id: number;
      quarantined: boolean;
      quarantine_reason: string | null;
      quarantine_expires_at: string | null;
    };
    expect(written.quarantined).toBe(true);
    expect(written.quarantine_reason).toBe("network: the payment gateway times out");
    expect(written.quarantine_expires_at).not.toBeNull();

    // After a reload the row is marked quarantined.
    await page.reload();
    const marked = page.getByRole("row").filter({ hasText: name });
    await expect(marked.getByTitle("Quarantined — runner may skip")).toBeVisible();
    // And the API agrees: the stored Test carries the flag, the reason and the expiry.
    const stored = (await (
      await page.request.get(`/api/tests/${written.id}/history?limit=1`)
    ).json()) as {
      test: { quarantined: boolean; quarantine_reason: string | null };
    };
    expect(stored.test.quarantined).toBe(true);
    expect(stored.test.quarantine_reason).toBe("network: the payment gateway times out");

    // Un-quarantining is reversible and leaves the seed as it was.
    await marked.getByRole("button", { name: "Un-quarantine" }).click();
    await expect(marked.getByTitle("Quarantined — runner may skip")).toHaveCount(0);
    const cleared = (await (
      await page.request.get(`/api/tests/${written.id}/history?limit=1`)
    ).json()) as {
      test: { quarantined: boolean; quarantine_reason: string | null };
    };
    expect(cleared.test.quarantined).toBe(false);
    expect(cleared.test.quarantine_reason).toBeNull();
  });

  test("search, sort, filters and the project picker change the rows and the URL", async ({
    page,
  }) => {
    await page.goto(DASH);
    await expect(testRows(page)).toHaveCount(4);

    // Search narrows the rows and the URL; loading that URL gives the same page.
    await page.getByLabel("Search").fill("checkout");
    await expect(page).toHaveURL(/q=checkout/);
    await expect(testRows(page)).toHaveCount(2);
    const searched = await rowNames(testRows(page));
    await page.goto(page.url());
    await expectRows(page, searched);
    await page.getByLabel("Search").fill("");
    await expect(page).not.toHaveURL(/q=/);
    await expect(testRows(page)).toHaveCount(4);

    // Sorting by Proven flakes promotes the Test with a same-SHA flip to second.
    await page.getByLabel("Sort by").selectOption("proven");
    await expect(page).toHaveURL(/sort=proven/);
    await expect(testRows(page).nth(1)).toContainText("test_checkout_total_rounding");
    const sorted = await rowNames(testRows(page));
    await page.goto(page.url());
    await expectRows(page, sorted);

    // Show stable Tests adds the six always-passing Tests and the broken one.
    await page.getByLabel("Show stable tests").check();
    await expect(page).toHaveURL(/stable=1/);
    await expect(testRows(page)).toHaveCount(10);
    const every = await rowNames(testRows(page));
    await page.goto(page.url());
    await expectRows(page, every);
    await page.getByLabel("Show stable tests").uncheck();

    // Broken on the Default branch is its own list, and it is in the stable tier.
    await page.getByLabel("Broken on default branch").check();
    await expect(page).toHaveURL(/broken=1/);
    const broken = testRows(page);
    await expect(broken).toHaveCount(1);
    await expect(broken).toContainText("test_schema_migration_v42");
    await expect(broken).toContainText("✕ broken on default branch");
    const brokenRows = await rowNames(testRows(page));
    await page.goto(page.url());
    await expectRows(page, brokenRows);
    await page.getByLabel("Broken on default branch").uncheck();

    // The project picker scopes the rows to one Project inside the Repo.
    await page.getByLabel("Project").selectOption("frontend");
    await expect(page).toHaveURL(/project=frontend/);
    const frontend = testRows(page);
    await expect(frontend).toHaveCount(2);
    await expect(frontend.filter({ hasText: "Cart > updates the badge" })).toHaveCount(1);
    await expect(frontend.filter({ hasText: "test_checkout_total_rounding" })).toHaveCount(0);
    const scoped = await rowNames(frontend);
    await page.goto(page.url());
    await expectRows(page, scoped);
  });

  test("the theme toggle changes data-theme and every font request answers 200", async ({
    page,
  }) => {
    const fonts: { path: string; status: number }[] = [];
    page.on("response", (r) => {
      const path = new URL(r.url()).pathname;
      if (/\.woff2?$/.test(path)) fonts.push({ path, status: r.status() });
    });
    await page.goto(DASH);
    await page.evaluate(() => document.fonts.ready);
    const root = page.locator("html");
    await expect(root).toHaveAttribute("data-theme", "dark");
    await page.getByRole("button", { name: "Switch to light theme" }).click();
    await expect(root).toHaveAttribute("data-theme", "light");
    await page.getByRole("button", { name: "Switch to dark theme" }).click();
    await expect(root).toHaveAttribute("data-theme", "dark");
    // The dashboard's own fonts are embedded in the binary: no CDN, no 404.
    expect(fonts.length).toBeGreaterThan(0);
    expect(fonts.filter((f) => f.status !== 200)).toEqual([]);
  });

  test("every mocked route answers with its fixture's field names", async ({ request }) => {
    // The `ui` project mocks these answers from `e2e/fixtures/*.json`. A field
    // this server has and a fixture does not (or the other way round) is a mock
    // that no longer describes the API, so compare the names against the seed.
    const routes: { fixture: string; probe: string }[] = [
      { fixture: "repos", probe: "/api/repos" },
      { fixture: "summary", probe: `/api/summary?repo=${REPO}` },
      { fixture: "tests", probe: `/api/tests?repo=${REPO}&page=1&page_size=50&sort=score` },
      // Page 2 has to exist here to have a row to read, so the probe pages the
      // list with the fixture's own page size of 50 rows over a total of 60.
      {
        fixture: "testsPage2",
        probe: `/api/tests?repo=${REPO}&page=2&page_size=4&sort=score&include_stable=true`,
      },
      { fixture: "testHistory", probe: "/api/tests/6/history?limit=60" },
      { fixture: "jobs", probe: `/api/jobs?repo=${REPO}&page=1&page_size=50&sort=score` },
      { fixture: "jobsSummary", probe: `/api/jobs/summary?repo=${REPO}` },
      { fixture: "jobHistory", probe: "/api/jobs/2/history?limit=60" },
      { fixture: "reportsSummary", probe: "/api/reports/summary" },
      { fixture: "reportsSummaryPending", probe: "/api/reports/summary" },
      // The queue route answers this fixture's shape, but the seed has no
      // failed Report to read a row from, so probe a Report of any status.
      { fixture: "reportsFailed", probe: "/api/reports?limit=20" },
      { fixture: "ciRunFailures", probe: `/api/ci-runs/run-12/failures?repo=${REPO}` },
    ];
    for (const { fixture, probe } of routes) {
      const answer = await request.get(probe);
      expect(answer.status(), `${probe} answered ${answer.status()}`).toBe(200);
      expect(
        answerFields(await answer.json()),
        `${fixture}.json has drifted from ${probe}`,
      ).toEqual(fixtureFields(fixture));
    }
  });
});
