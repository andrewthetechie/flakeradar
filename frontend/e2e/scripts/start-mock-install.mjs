// Start (and stop) the `e2e` Playwright project's mock install: a throwaway
// postgres:17-alpine, the real `flakeradar` binary with the built dashboard
// embedded, and the data `samples/simulate_ci.py` seeds. Nothing here reaches
// outside the machine — no GitHub token, so no issue filing; no Webhook URL, so
// no Notifications; no public origin.
//
// Playwright calls the default export as `globalSetup`, so the install is up and
// FULLY SEEDED before the first test navigates: the seeder posts 44 Reports and
// waits for the queue to drain, and `waitForSeeded` re-reads the counts the
// tests then assert. The returned function stops the binary and removes the
// container.
//
// Run it as a command to hold the same install for writing tests by hand:
//
//   node e2e/scripts/start-mock-install.mjs    # then Ctrl-C to stop
//
//   FLAKERADAR_BIN  the binary to run (default: newest build under target/)
//   E2E_CORE_PORT   the dashboard's port (default 8543)
//   E2E_PG_PORT     postgres's host port (default 15432)
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { request } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const FRONTEND = join(HERE, "..", "..");
const ROOT = join(FRONTEND, "..");
const PORT = Number(process.env.E2E_CORE_PORT ?? 8543);
const PG_PORT = Number(process.env.E2E_PG_PORT ?? 15432);
const HOST = "127.0.0.1";
const TOKEN = "e2e-install-token";
const PG_CONTAINER = "fr-e2e-pg";
const PG_IMAGE = "postgres:17-alpine";
const REPO = "demo/shop";

function requireInstall() {
  const bin =
    process.env.FLAKERADAR_BIN ??
    [
      join(ROOT, "target", "debug", "flakeradar"),
      join(ROOT, "target", "release", "flakeradar"),
    ].find((p) => existsSync(p));
  if (!bin) throw new Error("no flakeradar binary under target/ — run `cargo build` first");
  if (!existsSync(join(FRONTEND, "dist", "index.html"))) {
    throw new Error("frontend/dist has no index.html — run `npm run build` in frontend/ first");
  }
  return bin;
}

/** Run one program to completion; its stdout comes back trimmed. */
function run(program, args, { print = false } = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", stdio: print ? "inherit" : "pipe" });
  if (result.error) throw new Error(`${program}: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`${program} ${args.join(" ")} exited ${result.status}: ${result.stderr ?? ""}`);
  }
  return (result.stdout ?? "").trim();
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Wait until `check` answers truthy, or give up naming what it waited for. */
async function waitFor(check, label, attempts = 240) {
  for (let n = 0; n < attempts; n++) {
    // A server that is not ready yet reports a failed command, so anything
    // `check` throws is an answer of "not ready" and not a reason to give up.
    let ready = false;
    try {
      ready = Boolean(await check());
    } catch {
      ready = false;
    }
    if (ready) return;
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${label}`);
}

function getJson(path) {
  return new Promise((resolve, reject) => {
    const req = request({ host: HOST, port: PORT, path, method: "GET" }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () =>
        res.statusCode === 200
          ? resolve(JSON.parse(body))
          : reject(new Error(`${path} -> ${res.statusCode}`)),
      );
    });
    req.on("error", reject);
    req.end();
  });
}

async function startPostgres() {
  try {
    run("docker", ["rm", "-f", PG_CONTAINER]);
  } catch {
    // Nothing of ours to remove.
  }
  run("docker", [
    "run",
    "-d",
    "--name",
    PG_CONTAINER,
    "-e",
    "POSTGRES_USER=flakeradar",
    "-e",
    "POSTGRES_PASSWORD=flakeradar",
    "-e",
    "POSTGRES_DB=flakeradar",
    "-p",
    `${PG_PORT}:5432`,
    PG_IMAGE,
  ]);
  await waitFor(
    () =>
      spawnSync(
        "docker",
        ["exec", PG_CONTAINER, "pg_isready", "-U", "flakeradar", "-h", HOST, "-p", "5432"],
        { encoding: "utf8" },
      ).stdout?.includes("accepting connections"),
    `${PG_IMAGE} to accept connections`,
  );
}

/** The seeded state every test asserts: 10 Tests in the demo Repo, nothing queued. */
async function seeded() {
  const [summary, queue] = await Promise.all([
    getJson(`/api/summary?repo=${REPO}`),
    getJson("/api/reports/summary"),
  ]);
  return summary.total_tests === 10 && queue.pending === 0 && queue.failed === 0;
}

/** Boot the install, seed it, and answer the function that stops it again. */
export default async function startMockInstall() {
  const bin = requireInstall();
  await startPostgres();

  const core = spawn(bin, [], {
    cwd: ROOT,
    env: {
      ...process.env,
      FLAKERADAR_DATABASE_URL: `postgresql://flakeradar:flakeradar@${HOST}:${PG_PORT}/flakeradar`,
      FLAKERADAR_API_TOKEN: TOKEN,
      FLAKERADAR_HOST: HOST,
      FLAKERADAR_PORT: String(PORT),
      // No token, so no GitHub task; no Webhook URL, so no Notifications. Both
      // are left unset rather than set to nothing, so this install cannot reach
      // the network — and it needs no `FLAKERADAR_ALLOW_INSECURE`, because the
      // token above is not the shipped default.
      RUST_LOG: process.env.E2E_RUST_LOG ?? "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const lines = [];
  const collect = (where) => (chunk) => {
    lines.push(`[flakeradar] ${chunk}`.trimEnd());
    if (process.env.E2E_VERBOSE) process[where].write(`[flakeradar] ${chunk}`);
  };
  core.stdout.on("data", collect("stdout"));
  core.stderr.on("data", collect("stderr"));

  const stop = async () => {
    if (core.exitCode === null && core.signalCode === null) core.kill("SIGTERM");
    try {
      run("docker", ["rm", "-f", PG_CONTAINER]);
    } catch {
      // Already gone.
    }
  };

  try {
    await waitFor(async () => {
      await getJson("/api/health/ready");
      return true;
    }, "the server to answer /api/health/ready");
    // 14 CI runs of demo/shop: 43 Reports, then the worker drains the queue.
    run("python3", [join(ROOT, "samples", "simulate_ci.py"), `http://${HOST}:${PORT}`, TOKEN]);
    await waitFor(seeded, `the seed to show 10 Tests of ${REPO} with an empty queue`);
  } catch (error) {
    await stop();
    const log = lines.slice(-20).join("\n");
    throw new Error(`${error instanceof Error ? error.message : error}\n${log}`);
  }
  return stop;
}

// As a command, hold the install open until the terminal interrupts it.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const stop = await startMockInstall();
  console.log(`start-mock-install: dashboard ready at http://${HOST}:${PORT} — Ctrl-C to stop`);
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => void stop().then(() => process.exit(0)));
  }
  await new Promise(() => {});
}
