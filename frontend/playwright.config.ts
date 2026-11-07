import { defineConfig, type TestConfigWebServer } from "@playwright/test";

// Two projects, both against the built `frontend/dist`, both in Chromium:
//   `ui`  serves dist statically and answers every /api/* call with page.route
//         from fixtures, for states a real server rarely produces.
//   `e2e` starts the real binary with dist embedded plus a throwaway postgres,
//         seeded by samples/simulate_ci.py, so a browser drives the dashboard
//         that ships.
//
// `retries: 0` is the task's contract: three consecutive local runs of both
// projects must pass under CPU stress with no retries.
//
// One static server and one mock install, so the two `webServer` entries cannot
// both be started by one Playwright run — but Playwright 1.65 only reads
// `webServer` from the top-level config, never from a project (see
// `TestProject` in `playwright/types/test.d.ts`). So the entries are picked here
// from the `--project` flags: `--project=ui` must not boot a Docker container, a
// Rust binary and a seeded database, and a full run starts both.

const UI_PORT = 4173;
const E2E_PORT = 8543;

/** Project names named on the command line, or none when every project runs. */
function requestedProjects(): string[] {
  const argv = process.argv.slice(2);
  const names: string[] = [];
  argv.forEach((arg, i) => {
    const inline = /^--project=(.+)$/.exec(arg);
    if (inline) names.push(...inline[1].split(","));
    else if (arg === "--project" && argv[i + 1]) names.push(argv[i + 1]);
  });
  return names;
}

const requested = requestedProjects();
const runs = (project: string) => requested.length === 0 || requested.includes(project);

const servers: TestConfigWebServer[] = [];
if (runs("ui")) {
  servers.push({
    command: "node e2e/scripts/serve-dist.mjs",
    url: `http://127.0.0.1:${UI_PORT}`,
    // Locally a dashboard left up from a previous run is reused; CI never has one.
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  });
}
if (runs("e2e")) {
  servers.push({
    command: "node e2e/scripts/start-mock-install.mjs",
    url: `http://127.0.0.1:${E2E_PORT}/api/health/ready`,
    reuseExistingServer: !process.env.CI,
    // Cold start: pull postgres:17-alpine, migrate, then seed 43 reports.
    timeout: 600_000,
  });
}

export default defineConfig({
  testDir: "./e2e",
  retries: 0,
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI
    ? [["github"], ["html", { open: "never" }]]
    : [["line"], ["html", { open: "never" }]],
  outputDir: "test-results/playwright",
  webServer: servers,
  use: {
    // `retries: 0`, so "on-first-retry" would keep nothing: a failing test has
    // to leave a trace for CI to upload.
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "ui",
      testMatch: /ui\.spec\.ts/,
      use: { baseURL: `http://127.0.0.1:${UI_PORT}` },
    },
    {
      name: "e2e",
      testMatch: /e2e\.spec\.ts/,
      use: { baseURL: `http://127.0.0.1:${E2E_PORT}` },
    },
  ],
});
