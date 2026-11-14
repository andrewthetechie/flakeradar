// The Playwright quarantine helper. These first two cases check the pure part:
// the RegExp built from this sample's quarantine.json fixture, which is the
// pattern documented in the comment block above buildGrepInvert and asserted
// here character for character.
//
// The helper is TypeScript with only erasable syntax, so Node 22 or later runs
// this file as it is: Node strips the types, and no build step is involved.
//
//   node --test samples/quarantine/playwright/flakeradar-quarantine.test.mjs
//
// or in Docker with samples/quarantine/test.sh playwright, which runs the same
// command on Node 24, the Node of the sample's image.
//
// The five cases that follow drive the sample's example/ suite with the pinned
// @playwright/test, so they need it installed:
//
//   cd samples/quarantine/playwright && npm install
//
// The sample's Docker image installs it and runs this file as its default
// command, which is where these are judged.
//
// A local node:http server serves the body a case names, this sample's
// quarantine.json by default, and a case that uses it compares the decoded
// query, as ../README.md's Tests rule asks. Playwright runs as a child process
// whose cwd is example/, with FLAKERADAR_REPO=acme/app and FLAKERADAR_PROJECT=e2e,
// and every report and artifact goes to a temp dir, so a run leaves nothing under
// example/. The last two cases call the helper rather than the suite, in Node
// processes of their own: the helper's fetch is synchronous and would stop the
// loop that has to answer it. The last of those spawns its worker from the
// process that fetched, because that pair — a load and the worker that inherits
// its environment — is what the helper's export exists for.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { createServer as createNetServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { buildGrepInvert } from "./flakeradar-quarantine.ts";

const exampleDir = path.join(import.meta.dirname, "example");

const fixturePath = path.join(import.meta.dirname, "quarantine.json");
const fixtureText = readFileSync(fixturePath, "utf8");
const items = JSON.parse(fixtureText);

test("buildGrepInvert of the fixture is the documented RegExp", () => {
  // The same literal as the source in the comment block above buildGrepInvert.
  const documented =
    "(?:^| )cart\\.spec\\.ts cart adds an item(?=$| @)|(?:^| )cart\\.spec\\.ts checkout total(?=$| @)";
  assert.equal(buildGrepInvert(items).source, documented);
  assert.equal(buildGrepInvert(items).flags, "");

  assert.equal(buildGrepInvert([]), undefined);
});

test("the regexp matches the grep titles of the quarantined tests only", () => {
  // Playwright matches grepInvert against the space-joined title path and the
  // tags, so a project named unit gives these four greps.
  const pattern = buildGrepInvert(items);
  assert.equal(pattern.test(" unit cart.spec.ts cart adds an item"), true);
  assert.equal(pattern.test(" unit cart.spec.ts checkout total @slow"), true);
  assert.equal(pattern.test(" unit cart.spec.ts cart adds an item twice"), false);
  assert.equal(pattern.test(" unit cart.spec.ts shows the empty cart"), false);
});

/**
 * Serves a quarantine body on a localhost port and records every request.
 *
 * @param {string} [bodyText] the body for the first request; this sample's
 *   fixture by default
 * @param {string | null} [laterBodyText] the body for the requests after it, for
 *   a case whose list changes between two asks. null hangs up instead, so a case
 *   can make every ask after the first fail.
 * @returns {Promise<{url: string, requests: {path: string, query: URLSearchParams, apiKey: string | null}[], close: () => Promise<void>}>}
 */
async function serveFixture(bodyText = fixtureText, laterBodyText) {
  const requests = [];
  const server = createServer((request, response) => {
    // searchParams is the decoded form, so the case compares repo as acme/app.
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    requests.push({
      path: url.pathname,
      query: url.searchParams,
      apiKey: request.headers["x-api-key"] ?? null,
    });
    const answer = requests.length === 1 || laterBodyText === undefined ? bodyText : laterBodyText;
    if (answer === null) {
      request.destroy();
      return;
    }
    response.writeHead(200, { "Content-Length": String(Buffer.byteLength(answer)) });
    response.end(answer);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    requests,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

/**
 * The environment of a child that reads the helper's own variables: the sample's
 * repo and project, and nothing an earlier case left behind.
 *
 * @param {{[key: string]: string | undefined}} settings FLAKERADAR_URL and the
 *   like; undefined deletes the variable, so each case says what is set
 * @returns {{[key: string]: string}}
 */
function sampleEnv(settings) {
  const env = { ...process.env, FLAKERADAR_REPO: "acme/app", FLAKERADAR_PROJECT: "e2e" };
  delete env.GITHUB_REPOSITORY;
  // The helper's own channels: each case either sets them or starts clean.
  delete env.FLAKERADAR_QUARANTINE_CACHE;
  delete env.FLAKERADAR_QUARANTINE_CACHE_FILE;
  delete env.FLAKERADAR_QUARANTINE_REQUEST_URL;
  for (const [key, value] of Object.entries(settings)) {
    if (value === undefined) {
      delete env[key];
    } else {
      env[key] = value;
    }
  }
  return env;
}

/**
 * A child's exit code and its combined output, without the terminal's control
 * codes.
 *
 * @param {object} child a spawned process
 * @returns {Promise<{code: number | null, output: string}>}
 */
function finished(child) {
  let output = "";
  for (const stream of [child.stdout, child.stderr]) {
    stream.setEncoding("utf8");
    stream.on("data", (chunk) => {
      output += chunk;
    });
  }
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, output: output.replace(/\u001b\[[0-9;]*m/g, "") }));
  });
}

/**
 * Runs the pinned Playwright on the example suite, with the sample's
 * environment.
 *
 * @param {string[]} args Playwright's own arguments, starting with `test`
 * @param {{[key: string]: string | undefined}} settings FLAKERADAR_URL and the
 *   like; undefined deletes the variable, so each case says what is set
 * @returns {Promise<{code: number | null, output: string}>} the exit code and the
 *   combined stdout and stderr, without the terminal's control codes
 */
function runPlaywright(args, settings) {
  // A run's artifacts — the per-test dirs and .last-run.json — go to its output
  // dir, whose default is next to the nearest package.json. Naming a temp one
  // makes the answer the same in a checkout and in the image: nothing under
  // example/.
  const child = spawn("npx", ["playwright", ...args, "--output", reportPath("output")], {
    cwd: exampleDir,
    env: sampleEnv(settings),
  });
  return finished(child);
}

/**
 * A path in a fresh temp dir, for a report or the run's artifact dir: nothing is
 * written under example/, so a run leaves no artifact behind in the sample.
 *
 * @param {string} name the file's name, or the dir's
 * @returns {string}
 */
function reportPath(name) {
  return path.join(mkdtempSync(path.join(tmpdir(), "fr-q-")), name);
}

/**
 * Every `<testcase>` of a JUnit report as the (classname, name) pair the fixture
 * quotes, with the XML entities decoded.
 *
 * @param {string} report the file's text
 * @returns {{classname: string, name: string}[]}
 */
function junitTestcases(report) {
  const cases = [];
  for (const tag of report.match(/<testcase\b[^>]*>/g) ?? []) {
    const attribute = (key) => {
      const found = tag.match(new RegExp(`(?:^|\\s)${key}="([^"]*)"`));
      return found === null ? "" : decodeXmlEntities(found[1]);
    };
    cases.push({ classname: attribute("classname"), name: attribute("name") });
  }
  return cases;
}

/** @param {string} text an XML attribute value @returns {string} */
function decodeXmlEntities(text) {
  return text
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&#39;", "'")
    .replaceAll("&amp;", "&");
}

/**
 * The tests a `--list` run lists, one title path per line.
 *
 * A line is shaped `  [unit] › cart.spec.ts:14:7 › cart › adds an item`, so the
 * title is everything after the location field. Comparing whole titles is what
 * keeps `adds an item`, which is a prefix of `adds an item twice`, from matching
 * by accident.
 *
 * @param {string} output the combined output of a `--list` run
 * @returns {string[]} the titles, sorted
 */
function listedTests(output) {
  const titles = [];
  for (const line of output.split("\n")) {
    const found = line.match(/^ {2}\[unit\] \u203a \S+:\d+:\d+ \u203a (.+)$/);
    if (found !== null) {
      titles.push(found[1]);
    }
  }
  return titles.sort();
}

/**
 * A URL on a port that was bound and then closed, so connecting fails.
 *
 * @returns {Promise<string>}
 */
async function closedPortUrl() {
  const server = createNetServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return `http://127.0.0.1:${port}`;
}

/**
 * A valid body whose JSON is longer than one environment string may be, and still
 * small enough to be an answer rather than a stress test.
 *
 * Linux (MAX_ARG_STRLEN) refuses to start a process whose environment holds a
 * string of 128 KiB or more, so a cache this size must not go into the
 * environment the workers inherit: the run would die at the first `execve` rather
 * than skip anything. The two fixture items ride along at the front, so the
 * oversized list still holds something to skip.
 *
 * @returns {string} the body, as the endpoint would answer it
 */
function oversizedBody() {
  const body = items.map((item) => ({ ...item }));
  for (let index = 0; body.length < 900; index += 1) {
    body.push({
      suite: "bulk.spec.ts",
      classname: `bulk${index}.spec.ts`,
      name: `bulk \u203a test ${index}`,
      fingerprint: "0000000000000000000000000000000000000000",
      file: null,
      line: null,
      quarantined_at: "2026-11-01T12:00:00.000000Z",
      reason: null,
      expires_at: null,
    });
  }
  return JSON.stringify(body);
}

test("fixture names are what the junit reporter writes", async () => {
  // Off: with no URL nothing is fetched, so all four tests run and the report
  // holds the (classname, name) pairs the fixture quotes — the exact titles and
  // the exact " › " join the helper's pattern is built from.
  const report = reportPath("junit.xml");
  const result = await runPlaywright(["test", "--reporter=junit"], {
    FLAKERADAR_URL: undefined,
    FLAKERADAR_TOKEN: undefined,
    PLAYWRIGHT_JUNIT_OUTPUT_NAME: report,
  });
  assert.equal(result.code, 0, result.output);
  assert.doesNotMatch(result.output, /flakeradar-quarantine:/);

  const cases = junitTestcases(readFileSync(report, "utf8"));
  assert.equal(cases.length, 4, `Playwright reported all four tests: ${JSON.stringify(cases)}`);
  for (const item of items) {
    assert.ok(
      cases.some(
        (testCase) => testCase.classname === item.classname && testCase.name === item.name,
      ),
      `Playwright wrote the fixture pair ${item.classname} / ${item.name}: ${JSON.stringify(cases)}`,
    );
  }
}, { timeout: 120_000 });

test("quarantined tests are not run", async (t) => {
  const server = await serveFixture();
  t.after(() => server.close());
  const result = await runPlaywright(["test", "--list"], {
    FLAKERADAR_URL: server.url,
    FLAKERADAR_TOKEN: "secret",
  });
  assert.equal(result.code, 0, result.output);

  // The two survivors, and not one line more: `adds an item` is gone even though
  // `adds an item twice` stayed, and `checkout total` is gone with its tag.
  assert.deepEqual(listedTests(result.output), [
    "cart \u203a adds an item twice",
    "shows the empty cart",
  ]);
  assert.match(result.output, /Total: 2 tests in 1 file/);

  // The decoded query: repo is acme/app, not the acme%2Fapp that went on the
  // wire. A worker that read the cache asked no second question.
  assert.ok(server.requests.length >= 1, "Playwright fetched the quarantine list");
  for (const request of server.requests) {
    assert.equal(request.path, "/api/quarantine");
    assert.equal(request.query.get("repo"), "acme/app");
    assert.equal(request.query.get("project"), "e2e");
    assert.equal(request.apiKey, "secret");
  }
}, { timeout: 120_000 });

test("unreachable FlakeRadar filters nothing", async () => {
  const result = await runPlaywright(["test", "--list"], {
    FLAKERADAR_URL: await closedPortUrl(),
    FLAKERADAR_TOKEN: "secret",
  });
  // Fail open: the listing is green with all four tests, and stderr holds exactly
  // one contract line, not one per worker and not a decorated one.
  assert.equal(result.code, 0, result.output);
  assert.deepEqual(listedTests(result.output), [
    "cart \u203a adds an item",
    "cart \u203a adds an item twice",
    "checkout total",
    "shows the empty cart",
  ]);
  assert.match(result.output, /Total: 4 tests in 1 file/);
  const lines = result.output.match(/^flakeradar-quarantine: .*; skipping nothing$/gm) ?? [];
  assert.equal(lines.length, 1, result.output);
}, { timeout: 120_000 });

test("a cached list is reused without a fetch", async () => {
  const result = await runPlaywright(["test", "--list"], {
    FLAKERADAR_QUARANTINE_CACHE: fixtureText,
    FLAKERADAR_URL: await closedPortUrl(),
    FLAKERADAR_TOKEN: "secret",
  });
  // The cache is read before the server settings are, so the closed port stays
  // unasked: the same two tests as the fetched run, and no warning, because the
  // cache path never failed anything.
  assert.equal(result.code, 0, result.output);
  assert.deepEqual(listedTests(result.output), [
    "cart \u203a adds an item twice",
    "shows the empty cart",
  ]);
  assert.doesNotMatch(result.output, /flakeradar-quarantine:/);
}, { timeout: 120_000 });

test("a list too large for the environment skips and starts anyway", async (t) => {
  const body = oversizedBody();
  // The case is about the operating system's own limit: past it, an assignment
  // that succeeds in this process is an execve that fails in the next one. Still
  // an answer of a size the endpoint could write, not a stress test.
  assert.ok(Buffer.byteLength(body) > 128 * 1024, `${body.length} bytes is not oversized`);
  assert.ok(Buffer.byteLength(body) < 1024 * 1024, `${body.length} bytes is not a body`);

  // The server answers the first ask and hangs up on every later one, which is
  // how a worker would find FlakeRadar once the main process has asked.
  const server = await serveFixture(body, null);
  t.after(() => server.close());
  // A full run and not a listing: starting a Playwright worker is what a cache
  // too large for the inherited environment breaks.
  const result = await runPlaywright(["test"], {
    FLAKERADAR_URL: server.url,
    FLAKERADAR_TOKEN: "secret",
  });
  // The worker started, and the oversized list still skipped its two tests: the
  // two survivors of the fixture ran and passed.
  assert.doesNotMatch(result.output, /E2BIG/, result.output);
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /2 passed/, result.output);
  // The oversized list travelled to the worker some other way than the
  // environment, so the worker asked nothing and had nothing to fail: one ask for
  // the run, and not one contract line, which a failed ask in the worker writes.
  assert.equal(server.requests.length, 1, `requests: ${JSON.stringify(server.requests)}`);
  assert.doesNotMatch(result.output, /flakeradar-quarantine:/, result.output);
}, { timeout: 120_000 });

/**
 * Calls the helper twice, in a Node process of its own, and reports what it saw.
 *
 * Not in this one: the helper's fetch is synchronous, and this process is the one
 * that answers the request, so a fetch here would wait for a server its own
 * stopped event loop cannot run. And not through Playwright, which calls the
 * helper once per config load: one call too few to compare.
 *
 * @param {string} url the server to ask
 * @returns {Promise<{code: number | null, answer: object | null, output: string}>}
 *   the exit code, the child's one ANSWER line, and its output
 */
async function callHelperTwice(url) {
  const script = [
    'const { quarantineGrepInvert } = await import("./flakeradar-quarantine.ts");',
    "const first = quarantineGrepInvert();",
    "const second = quarantineGrepInvert();",
    'process.stdout.write("ANSWER " + JSON.stringify({',
    "  hasPattern: first !== undefined,",
    "  sameSource: first !== undefined && second !== undefined && first.source === second.source,",
    '  cacheBytes: (process.env.FLAKERADAR_QUARANTINE_CACHE ?? "").length,',
    '}) + "\\n");',
  ].join("\n");
  const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
    cwd: import.meta.dirname,
    env: sampleEnv({ FLAKERADAR_URL: url, FLAKERADAR_TOKEN: "secret" }),
  });
  const result = await finished(child);
  const line = result.output.match(/^ANSWER (.*)$/m);
  return {
    code: result.code,
    answer: line === null ? null : JSON.parse(line[1]),
    output: result.output,
  };
}

test("an oversized list is kept in the process, not in the environment", async (t) => {
  const server = await serveFixture(oversizedBody());
  t.after(() => server.close());
  const run = await callHelperTwice(server.url);
  assert.equal(run.code, 0, run.output);
  assert.notEqual(run.answer, null, run.output);
  // The oversized list is a pattern, and the second call has the same one.
  assert.equal(run.answer.hasPattern, true, "the oversized list skips the fixture's tests");
  assert.equal(run.answer.sameSource, true, "the second call read the same list");
  // The environment the workers inherit never held the oversized value. That is
  // the bug this case is about: on Linux the assignment does not fail, and the
  // execve of the next process — a Playwright worker — is what dies with E2BIG.
  assert.ok(
    run.answer.cacheBytes < 128 * 1024,
    `the environment holds ${run.answer.cacheBytes} bytes of cache`,
  );
  // One question for the two calls: the second read what the first fetched.
  assert.equal(server.requests.length, 1, `requests: ${JSON.stringify(server.requests)}`);
}, { timeout: 120_000 });

/** The example suite's four tests in the string Playwright matches grepInvert against. */
const grepTitles = [
  " unit cart.spec.ts cart adds an item",
  " unit cart.spec.ts cart adds an item twice",
  " unit cart.spec.ts checkout total @slow",
  " unit cart.spec.ts shows the empty cart",
];

/** The two of them the fixture's items quarantine. */
const fixtureSkips = [grepTitles[0], grepTitles[2]];

/**
 * What a process does with the helper: build its pattern, and note the tests it
 * skips and the cache this process exported for the ones it spawns.
 *
 * @returns {string} the shared head of both scripts of callHelperWithWorker
 */
function helperCallScript() {
  return [
    `const titles = ${JSON.stringify(grepTitles)};`,
    'const { quarantineGrepInvert } = await import("./flakeradar-quarantine.ts");',
    "const pattern = quarantineGrepInvert();",
    "const skipped = titles.filter((title) => pattern !== undefined && pattern.test(title));",
    "const exported = {",
    '  cacheBytes: (process.env.FLAKERADAR_QUARANTINE_CACHE ?? "").length,',
    '  cacheFile: process.env.FLAKERADAR_QUARANTINE_CACHE_FILE ?? "",',
    "};",
  ].join("\n");
}

/**
 * Calls the helper in a Node process that then spawns a second one, which is how
 * Playwright's main process hands its list to a worker: the worker inherits the
 * environment, and it must apply the same list even though it could ask again.
 *
 * Two processes of their own, both because this one answers the fetch and a
 * synchronous fetch here would wait on a server its own stopped event loop cannot
 * run, and because the exported file belongs to the process that wrote it.
 *
 * @param {string} url the server to ask
 * @returns {Promise<{code: number | null, answer: object | null, output: string}>}
 *   the exit code, the initial process's one ANSWER line — with the worker's
 *   inside it — and the initial process's output
 */
async function callHelperWithWorker(url) {
  const workerScript = [
    helperCallScript(),
    'process.stdout.write("WORKER " + JSON.stringify({ skipped, ...exported }) + "\\n");',
  ].join("\n");
  const script = [
    'const { spawnSync } = await import("node:child_process");',
    `const workerScript = ${JSON.stringify(workerScript)};`,
    helperCallScript(),
    "const worker = spawnSync(process.execPath, ['--input-type=module', '-e', workerScript], {",
    "  cwd: process.cwd(),",
    "  env: process.env,",
    '  encoding: "utf8",',
    "});",
    'process.stdout.write("ANSWER " + JSON.stringify({',
    "  initial: { skipped, ...exported },",
    '  worker: JSON.parse((`${worker.stdout}${worker.stderr}`.match(/^WORKER (.*)$/m) ?? [])[1] ?? "null"),',
    "  workerExit: worker.status,",
    '  workerOutput: `${worker.stdout}${worker.stderr}`,',
    '}) + "\\n");',
  ].join("\n");
  const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
    cwd: import.meta.dirname,
    env: sampleEnv({ FLAKERADAR_URL: url, FLAKERADAR_TOKEN: "secret" }),
  });
  const result = await finished(child);
  const line = result.output.match(/^ANSWER (.*)$/m);
  return {
    code: result.code,
    answer: line === null ? null : JSON.parse(line[1]),
    output: result.output,
  };
}

test("an oversized list reaches the processes this one spawns", async (t) => {
  // The first ask gets the oversized list and a later one a different, shorter
  // list: whatever a second process applies, it must be the list the first one
  // already skipped with, and not whatever the server happens to answer now.
  const server = await serveFixture(oversizedBody(), JSON.stringify([items[1]]));
  t.after(() => server.close());

  const run = await callHelperWithWorker(server.url);
  assert.equal(run.code, 0, run.output);
  assert.notEqual(run.answer, null, run.output);
  assert.notEqual(run.answer.worker, null, run.answer.workerOutput);
  assert.deepEqual(run.answer.initial.skipped, fixtureSkips);
  assert.equal(run.answer.workerExit, 0, run.answer.workerOutput);

  // The worker skipped what the first process skipped, so the tests quarantined by
  // the first answer stayed quarantined: it read the exported list where a list
  // kept in one process's memory alone leaves it to ask the server for itself.
  assert.deepEqual(run.answer.worker.skipped, fixtureSkips);
  // One ask for both processes, and nothing to fail open over.
  assert.equal(server.requests.length, 1, `requests: ${JSON.stringify(server.requests)}`);
  assert.doesNotMatch(run.answer.workerOutput, /flakeradar-quarantine:/, run.answer.workerOutput);
  // The oversized list never went into the inherited environment, where the next
  // execve would have died: the worker read it from a short variable's path.
  assert.ok(
    run.answer.initial.cacheBytes < 128 * 1024 && run.answer.worker.cacheBytes < 128 * 1024,
    `the environment holds ${run.answer.worker.cacheBytes} bytes of cache`,
  );
  assert.notEqual(run.answer.worker.cacheFile, "", "the list travelled as a file path");
}, { timeout: 120_000 });
