// One command to start (and stop) the `e2e` Playwright project's mock install:
// a throwaway postgres:17-alpine, the real `flakeradar` binary with the built
// dashboard embedded, and the data `samples/simulate_ci.py` seeds. Nothing here
// reaches outside the machine: no GitHub token, no Webhook URL, no public origin.
//
// Playwright runs this as the project's server and SIGTERMs it when the tests
// finish; the signal handler stops the binary and removes the container.
//
//   FLAKERADAR_BIN   the binary to run (default: the newest build under target/)
//   FLAKERADAR_DIST  the dashboard to embed (default: frontend/dist, checked below)
//   E2E_CORE_PORT    the dashboard's port (default 8543)
//   E2E_PG_PORT      postgres's host port (default 15432)
import { spawn, spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { request } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const FRONTEND = join(HERE, "..", "..");
const ROOT = join(FRONTEND, "..");
const PORT = Number(process.env.E2E_CORE_PORT ?? 8543);
const PG_PORT = Number(process.env.E2E_PG_PORT ?? 15432);
const HOST = "127.0.0.1";
const TOKEN = "e2e-install-token";
const PG_CONTAINER = "fr-e2e-pg";
const PG_IMAGE = "postgres:17-alpine";

const BIN =
  process.env.FLAKERADAR_BIN ??
  [join(ROOT, "target", "debug", "flakeradar"), join(ROOT, "target", "release", "flakeradar")].find(
    (p) => existsSync(p),
  );
if (!BIN) {
  fail("no flakeradar binary under target/ — run `cargo build` first");
}
if (!existsSync(join(FRONTEND, "dist", "index.html"))) {
  fail("frontend/dist has no index.html — run `npm run build` in frontend/ first");
}

function fail(message) {
  console.error(`start-mock-install: ${message}`);
  process.exit(1);
}

/** Run one program to completion; its stdout is returned trimmed. */
function run(program, args, { print = false } = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", stdio: print ? "inherit" : "pipe" });
  if (result.error) throw new Error(`${program}: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`${program} ${args.join(" ")} exited ${result.status}: ${result.stderr ?? ""}`);
  }
  return (result.stdout ?? "").trim();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Wait until `check` answers truthy, or give up naming what it waited for. */
async function waitFor(check, label, attempts = 240) {
  for (let n = 0; n < attempts; n++) {
    let answer;
    try {
      answer = await check();
    } catch {
      answer = false;
    }
    if (answer) return;
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${label}`);
}

function ready(path) {
  return new Promise((resolve) => {
    const req = request({ host: HOST, port: PORT, path, method: "GET" }, (res) => {
      res.resume();
      resolve(res.statusCode >= 200 && res.statusCode < 300);
    });
    req.on("error", () => resolve(false));
    req.end();
  });
}

async function startPostgres() {
  try {
    run("docker", ["rm", "-f", PG_CONTAINER], { print: false });
  } catch {
    // No container of ours to remove.
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
      run("docker", [
        "exec",
        PG_CONTAINER,
        "pg_isready",
        "-U",
        "flakeradar",
        "-h",
        "127.0.0.1",
        "-p",
        "5432",
      ]).includes("accepting connections"),
    `${PG_IMAGE} to accept connections`,
  );
}

async function main() {
  let core = null;
  const stop = () => {
    if (core && core.exitCode === null && core.signalCode === null) core.kill("SIGTERM");
    try {
      run("docker", ["rm", "-f", PG_CONTAINER]);
    } catch {
      // Already gone.
    }
  };
  process.on("SIGTERM", () => stop());
  process.on("SIGINT", () => stop());
  process.on("exit", stop);

  await startPostgres();

  core = spawn(BIN, [], {
    cwd: ROOT,
    env: {
      ...process.env,
      FLAKERADAR_DATABASE_URL: `postgresql://flakeradar:flakeradar@${HOST}:${PG_PORT}/flakeradar`,
      FLAKERADAR_API_TOKEN: TOKEN,
      FLAKERADAR_HOST: HOST,
      FLAKERADAR_PORT: String(PORT),
      // No token, so no GitHub task; no Webhook URL, so no Notifications. Both
      // are left unset rather than set to nothing, so this install cannot reach
      // the network and needs no `FLAKERADAR_ALLOW_INSECURE` either: the token
      // above is not the shipped default.
      RUST_LOG: process.env.E2E_RUST_LOG ?? "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const prefix = (stream) => (chunk) => stream.write(`[flakeradar] ${chunk}`);
  core.stdout.on("data", prefix(process.stdout));
  core.stderr.on("data", prefix(process.stderr));
  core.on("exit", (code, signal) => {
    console.error(`start-mock-install: flakeradar exited (${code ?? signal})`);
  });

  await waitFor(() => ready("/api/health/ready"), "the server to answer /api/health/ready");

  // Seed the demo history, then wait for the worker: the seeder itself polls
  // /api/reports/summary until nothing is pending.
  run("python3", [join(ROOT, "samples", "simulate_ci.py"), `http://${HOST}:${PORT}`, TOKEN], {
    print: true,
  });
  console.log(`start-mock-install: dashboard ready at http://${HOST}:${PORT}`);

  // Stay up until Playwright stops this process.
  await new Promise(() => {});
}

main().catch((error) => {
  console.error(`start-mock-install: ${error instanceof Error ? error.message : error}`);
  process.exit(1);
});
