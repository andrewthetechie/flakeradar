// The Vitest and Jest quarantine helper. The first two cases check the pure part:
// the pattern built from this sample's vitest/quarantine.json fixture, which is
// the RegExp documented in the comment block above buildTestNamePattern and
// asserted here character for character.
//
// The four runner cases drive the real runners, so they need the pinned ones:
//
//   cd samples/quarantine/vitest-jest && npm install
//
// Then, from any directory, on Node 18 or later:
//
//   node --test flakeradar-quarantine.test.mjs
//
// or in Docker with samples/quarantine/test.sh vitest-jest, which installs them.
// That image runs Node 24, so it is where these four cases are judged; they also
// pass on Node 22, the Node of a plain sandbox.
//
// A local node:http server serves one fixture per case, and the case compares the
// decoded query, as ../README.md's Tests rule asks. Each runner runs as a child
// process whose cwd is example/, with FLAKERADAR_REPO=acme/app and
// FLAKERADAR_PROJECT=unit; every report goes to a temp dir, never under
// example/.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { createServer as createNetServer } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { buildTestNamePattern } from "./flakeradar-quarantine.mjs";

const exampleDir = path.join(import.meta.dirname, "example");

const fixturePath = path.join(import.meta.dirname, "vitest", "quarantine.json");
const items = JSON.parse(readFileSync(fixturePath, "utf8"));

test("buildTestNamePattern of the vitest fixture is the documented RegExp", () => {
  // The same literal as the source in the comment block above buildTestNamePattern.
  const documented = "^(?!(?:cart > adds an item|checkout total)$)";
  assert.equal(buildTestNamePattern(items).source, documented);

  assert.equal(buildTestNamePattern([]), undefined);

  // Two items with the same name give one alternative, not two.
  const doubled = buildTestNamePattern([
    { classname: "cart.test.js", name: "cart > adds an item" },
    { classname: "cart.test.js", name: "cart > adds an item" },
  ]);
  assert.equal(doubled.source.split("|").length, 1);
});

test("the pattern excludes exact names only", () => {
  const pattern = buildTestNamePattern(items);
  assert.equal(pattern.test("cart > adds an item"), false);
  assert.equal(pattern.test("checkout total"), false);
  assert.equal(pattern.test("cart > adds an item twice"), true);
  assert.equal(pattern.test("shows the empty cart"), true);
});

/**
 * Reads one of the sample's fixtures, the body the server answers.
 *
 * @param {string} name the fixture's dir, `vitest` or `jest`
 * @returns {any[]} the items, in list order
 */
function readFixture(name) {
  return JSON.parse(readFileSync(path.join(import.meta.dirname, name, "quarantine.json"), "utf8"));
}

/**
 * Serves one fixture on a localhost port and records every request it saw.
 *
 * @param {string} name the fixture's dir, `vitest` or `jest`
 * @returns {Promise<{url: string, requests: {path: string, query: URLSearchParams, apiKey: string | null}[], close: () => Promise<void>}>}
 */
async function serveFixture(name) {
  const body = readFileSync(path.join(import.meta.dirname, name, "quarantine.json"), "utf8");
  const requests = [];
  const server = createServer((request, response) => {
    // searchParams is the decoded form, so the case compares repo as acme/app.
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    requests.push({
      path: url.pathname,
      query: url.searchParams,
      apiKey: request.headers["x-api-key"] ?? null,
    });
    response.writeHead(200, { "Content-Length": String(Buffer.byteLength(body)) });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    requests,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

/**
 * Runs one pinned runner in example/, with the sample's environment.
 *
 * @param {string} command `vitest` or `jest`, the bin npx finds in the sample
 * @param {string[]} args the runner's own arguments
 * @param {{[key: string]: string | undefined}} settings FLAKERADAR_URL and the
 *   like; undefined deletes the variable, so each case says what is set
 * @returns {Promise<{code: number | null, output: string}>} the exit code and the
 *   combined stdout and stderr, without the terminal's control codes
 */
function runRunner(command, args, settings) {
  const env = { ...process.env, FLAKERADAR_REPO: "acme/app", FLAKERADAR_PROJECT: "unit" };
  delete env.GITHUB_REPOSITORY;
  for (const [key, value] of Object.entries(settings)) {
    if (value === undefined) {
      delete env[key];
    } else {
      env[key] = value;
    }
  }
  const child = spawn("npx", [command, ...args], { cwd: exampleDir, env });
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
 * A path in a fresh temp dir, for a runner's report: nothing is written under
 * example/, so a run leaves no artifact behind in the sample.
 *
 * @param {string} name the report's file name
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
 * What a runner's JSON report says ran. Both reporters use the same shape here:
 * `testResults` with `assertionResults`, whose `fullName` is the joined names
 * that the pattern matched, and whose status is `passed` only for a test that
 * ran — a filtered test is `skipped` in Vitest and `pending` in Jest.
 *
 * @param {string} file
 * @returns {{passed: string[], other: string[], numPassedTests: number}}
 */
function jsonReport(file) {
  const report = JSON.parse(readFileSync(file, "utf8"));
  const passed = [];
  const other = [];
  for (const result of report.testResults) {
    for (const assertion of result.assertionResults) {
      (assertion.status === "passed" ? passed : other).push(assertion.fullName);
    }
  }
  return { passed: passed.sort(), other: other.sort(), numPassedTests: report.numPassedTests };
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

test("fixture names are what each junit reporter writes", async () => {
  // Off: with no URL nothing is fetched, so each runner reports all four tests.
  const vitestReport = reportPath("vitest-junit.xml");
  const vitest = await runRunner("vitest", ["run", "--reporter=junit", `--outputFile=${vitestReport}`], {
    FLAKERADAR_URL: undefined,
    FLAKERADAR_TOKEN: undefined,
  });
  assert.equal(vitest.code, 0, vitest.output);
  assert.doesNotMatch(vitest.output, /flakeradar-quarantine:/);

  const jestReport = reportPath("jest-junit.xml");
  const jest = await runRunner("jest", ["--reporters=jest-junit"], {
    FLAKERADAR_URL: undefined,
    FLAKERADAR_TOKEN: undefined,
    JEST_JUNIT_OUTPUT_FILE: jestReport,
  });
  assert.equal(jest.code, 0, jest.output);
  assert.doesNotMatch(jest.output, /flakeradar-quarantine:/);

  for (const [runner, file] of [
    ["vitest", vitestReport],
    ["jest", jestReport],
  ]) {
    const cases = junitTestcases(readFileSync(file, "utf8"));
    assert.equal(cases.length, 4, `${runner} reported all four tests: ${JSON.stringify(cases)}`);
    for (const item of readFixture(runner)) {
      assert.ok(
        cases.some((testCase) => testCase.classname === item.classname && testCase.name === item.name),
        `${runner} wrote the fixture pair ${item.classname} / ${item.name}: ${JSON.stringify(cases)}`,
      );
    }
  }
}, { timeout: 120_000 });

test("vitest skips quarantined tests", async (t) => {
  const server = await serveFixture("vitest");
  t.after(() => server.close());
  const report = reportPath("vitest.json");
  const result = await runRunner("vitest", ["run", "--reporter=json", `--outputFile=${report}`], {
    FLAKERADAR_URL: server.url,
    FLAKERADAR_TOKEN: "secret",
  });
  assert.equal(result.code, 0, result.output);

  // The two names that ran are exactly the two the fixture does not quote. The
  // json reporter joins the names with a space in fullName while the pattern it
  // filtered on saw Vitest's " > " join, so the skipped pair is quoted the way
  // the report spells it — and the runner still skipped exactly the fixture.
  const { passed, other, numPassedTests } = jsonReport(report);
  assert.deepEqual(passed, ["cart adds an item twice", "shows the empty cart"]);
  assert.equal(numPassedTests, 2);
  assert.deepEqual(
    readFixture("vitest")
      .map((item) => item.name.replace(" > ", " "))
      .sort(),
    other,
  );

  // The decoded query: repo is acme/app, not the acme%2Fapp that went on the wire.
  assert.equal(server.requests.length, 1);
  const [request] = server.requests;
  assert.equal(request.path, "/api/quarantine");
  assert.equal(request.query.get("repo"), "acme/app");
  assert.equal(request.query.get("project"), "unit");
  assert.equal(request.apiKey, "secret");
}, { timeout: 120_000 });

test("jest skips quarantined tests", async (t) => {
  const server = await serveFixture("jest");
  t.after(() => server.close());
  const report = reportPath("jest.json");
  const result = await runRunner("jest", ["--json", `--outputFile=${report}`], {
    FLAKERADAR_URL: server.url,
    FLAKERADAR_TOKEN: "secret",
  });
  assert.equal(result.code, 0, result.output);

  // The same two tests pass. Jest's fullName is the space-joined names, which is
  // the trimmed form of what the fixture holds, and it keeps the filtered test in
  // the report as pending rather than dropping it.
  const { passed, other, numPassedTests } = jsonReport(report);
  assert.deepEqual(passed, ["cart adds an item twice", "shows the empty cart"]);
  assert.equal(numPassedTests, 2);
  assert.deepEqual(
    readFixture("jest")
      .map((item) => item.name.trim())
      .sort(),
    other,
  );

  // Jest loads its config in the main process and again in the worker, so it
  // asks more than once; every ask carries the same decoded query.
  assert.ok(server.requests.length >= 1, "Jest fetched the quarantine list");
  for (const request of server.requests) {
    assert.equal(request.path, "/api/quarantine");
    assert.equal(request.query.get("repo"), "acme/app");
    assert.equal(request.query.get("project"), "unit");
    assert.equal(request.apiKey, "secret");
  }
}, { timeout: 120_000 });

test("unreachable FlakeRadar skips nothing", async () => {
  const result = await runRunner("vitest", ["run"], {
    FLAKERADAR_URL: await closedPortUrl(),
    FLAKERADAR_TOKEN: "secret",
  });
  // Fail open: the run is green with all four tests, and stderr holds exactly
  // one contract line, not one per worker and not a decorated one.
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /Tests\s+4 passed \(4\)/);
  const lines = result.output.match(/^flakeradar-quarantine: .*; skipping nothing$/gm) ?? [];
  assert.equal(lines.length, 1, result.output);
}, { timeout: 120_000 });
