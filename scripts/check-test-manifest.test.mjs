import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkManifest,
  lastDoneTask,
  parseCsv,
  parseManifest,
  parseNextestList,
} from "./check-test-manifest.mjs";

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), "check-test-manifest.mjs");
const REPO_ROOT = path.join(path.dirname(SCRIPT), "..");

const HEADER = "pytest_id,task,status,rust_test,reason";

const fixtureDirs = [];
process.on("exit", () => {
  for (const dir of fixtureDirs) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/**
 * A throwaway tree holding the manifest and STATUS.md the checker reads, plus a
 * `bin/cargo` shim on PATH that prints the given nextest list document.
 */
function fixtureRoot({ manifest, status, nextest }) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "manifest-fixture-"));
  fixtureDirs.push(root);
  const planDir = path.join(root, "docs", "plans", "rust-rewrite");
  fs.mkdirSync(planDir, { recursive: true });
  fs.writeFileSync(path.join(planDir, "test-manifest.csv"), manifest);
  if (status !== undefined) {
    fs.writeFileSync(path.join(planDir, "STATUS.md"), status);
  }
  const bin = path.join(root, "bin");
  if (nextest !== undefined) {
    fs.mkdirSync(bin);
    const cargo = path.join(bin, "cargo");
    fs.writeFileSync(cargo, `#!/bin/sh\nprintf '%s' '${nextest}'\n`);
    fs.chmodSync(cargo, 0o755);
  }
  return { root, bin };
}

function runChecker(args, { root, bin } = {}) {
  const env = { ...process.env };
  if (bin) env.PATH = `${bin}${path.delimiter}${env.PATH}`;
  return spawnSync(process.execPath, [SCRIPT, ...args, ...(root ? ["--root", root] : [])], {
    encoding: "utf8",
    env,
  });
}

// ── parseCsv ───────────────────────────────────────────────────────────────

test("parseCsv splits plain rows on commas and newlines", () => {
  assert.deepEqual(parseCsv("a,b\nc,d\n"), [
    ["a", "b"],
    ["c", "d"],
  ]);
});

test("parseCsv keeps a quoted comma inside one field", () => {
  assert.deepEqual(parseCsv('a,"b,c",d\n'), [["a", "b,c", "d"]]);
});

test("parseCsv unescapes a doubled quote", () => {
  assert.deepEqual(parseCsv('a,"say ""hi""",c\n'), [["a", 'say "hi"', "c"]]);
});

test("parseCsv accepts CRLF line endings", () => {
  assert.deepEqual(parseCsv("a,b\r\nc,d\r\n"), [
    ["a", "b"],
    ["c", "d"],
  ]);
});

test("parseCsv keeps an empty trailing field", () => {
  assert.deepEqual(parseCsv("a,,\n"), [["a", "", ""]]);
});

// ── parseManifest ──────────────────────────────────────────────────────────

test("parseManifest reads the five columns and the line number", () => {
  const { rows, problems } = parseManifest(`${HEADER}\npkg::test_one,4,ported,parsing::one,`);
  assert.deepEqual(problems, []);
  assert.deepEqual(rows, [
    { line: 2, pytestId: "pkg::test_one", task: 4, status: "ported", rustTest: "parsing::one", reason: "" },
  ]);
});

test("parseManifest reports an empty manifest", () => {
  assert.deepEqual(parseManifest(""), { rows: [], problems: ["the manifest is empty"] });
});

test("parseManifest reports a wrong header", () => {
  const { problems } = parseManifest("pytest_id,task\n");
  assert.match(problems[0], /header is "pytest_id,task", expected/);
});

test("parseManifest reports a row with the wrong column count", () => {
  const { rows, problems } = parseManifest(`${HEADER}\npkg::test_one,4,ported\n`);
  assert.deepEqual(rows, []);
  assert.match(problems[0], /line 2: 3 columns, expected 5/);
});

test("parseManifest reports an unknown status", () => {
  const { rows, problems } = parseManifest(`${HEADER}\npkg::test_one,4,done,,\n`);
  assert.deepEqual(rows, []);
  assert.match(problems[0], /status "done" is not one of pending, ported, na, differs/);
});

test("parseManifest reports a non-numeric task", () => {
  const { rows, problems } = parseManifest(`${HEADER}\npkg::test_one,four,na,,reason\n`);
  assert.deepEqual(rows, []);
  assert.match(problems[0], /task "four" is not a number/);
});

// ── lastDoneTask ───────────────────────────────────────────────────────────

const STATUS = `# Rust rewrite status

| Task | Title | State | Commit | Notes |
|---|---|---|---|---|
| 01 | First | Done | abc123 | |
| 02 | Second | Done | def456 | |
| 03 | Third | Pending | | |
| 04 | Fourth | Blocked | | BLOCKED: waiting |
`;

test("lastDoneTask returns the highest task marked Done", () => {
  assert.equal(lastDoneTask(STATUS), 2);
});

test("lastDoneTask ignores Pending and Blocked rows", () => {
  const onlyPending = STATUS.replace("| 01 | First | Done |", "| 01 | First | Pending |");
  assert.equal(lastDoneTask(onlyPending), 2);
});

test("lastDoneTask returns null when no task is done", () => {
  assert.equal(lastDoneTask("| Task | Title | State | Commit | Notes |\n|---|---|---|---|---|\n"), null);
});

// ── checkManifest ──────────────────────────────────────────────────────────

const row = (task, status, extra = {}) => ({
  line: 2,
  pytestId: `pkg::test_${task}`,
  task,
  status,
  rustTest: "",
  reason: "",
  ...extra,
});

test("checkManifest passes a ported row whose test nextest knows", () => {
  const rows = [row(4, "ported", { rustTest: "parsing::one" })];
  assert.deepEqual(checkManifest(rows, 4, new Set(["parsing::one"])), []);
});

test("checkManifest flags a pending row in scope", () => {
  const problems = checkManifest([row(4, "pending")], 4, new Set());
  assert.deepEqual(problems, ["pkg::test_4: task 4 has landed but this row is still pending"]);
});

test("checkManifest ignores rows of a task that has not landed", () => {
  assert.deepEqual(checkManifest([row(9, "pending")], 4, new Set()), []);
});

test("checkManifest flags an na row without a reason", () => {
  const problems = checkManifest([row(4, "na", { reason: "  " })], 4, new Set());
  assert.deepEqual(problems, ["pkg::test_4: marked na without a reason"]);
});

test("checkManifest accepts an na row with a reason", () => {
  assert.deepEqual(checkManifest([row(4, "na", { reason: "Python-only harness" })], 4, new Set()), []);
});

test("checkManifest accepts a differs row with a reason", () => {
  const rows = [row(4, "differs", { reason: "differences.md: line attribute" })];
  assert.deepEqual(checkManifest(rows, 4, new Set()), []);
});

test("checkManifest flags a differs row without a reason", () => {
  const problems = checkManifest([row(4, "differs")], 4, new Set());
  assert.deepEqual(problems, ["pkg::test_4: marked differs without a reason"]);
});

test("checkManifest flags a ported row without a rust_test", () => {
  const problems = checkManifest([row(4, "ported")], 4, new Set());
  assert.deepEqual(problems, ["pkg::test_4: marked ported without a rust_test"]);
});

test("checkManifest flags a ported row naming a test nextest does not have", () => {
  const problems = checkManifest([row(4, "ported", { rustTest: "parsing::missing" })], 4, new Set(["other"]));
  assert.match(problems[0], /rust_test "parsing::missing" is not a test nextest knows/);
});

test("checkManifest skips the nextest lookup when it was not asked for one", () => {
  const rows = [row(4, "ported", { rustTest: "parsing::missing" })];
  assert.deepEqual(checkManifest(rows, 4, null), []);
});

// ── parseNextestList ───────────────────────────────────────────────────────

test("parseNextestList collects plain and suite-qualified test names", () => {
  const doc = JSON.stringify({
    "rust-suites": {
      "flakeradar::parsing": { testcases: { t_one: {}, t_two: {} } },
    },
    "test-count": 2,
  });
  const names = parseNextestList(doc);
  assert.equal(names.has("t_one"), true);
  assert.equal(names.has("flakeradar::parsing::t_two"), true);
});

test("parseNextestList returns an empty set for unreadable output", () => {
  assert.equal(parseNextestList("not json").size, 0);
});

// ── CLI ────────────────────────────────────────────────────────────────────

test("the CLI passes against the committed manifest at the last done task", () => {
  const run = runChecker([]);
  assert.equal(run.status, 0);
  assert.match(run.stdout, /Test manifest check passed\./);
});

test("the CLI passes once every landed task owns no rows", () => {
  const run = runChecker(["--upto", "2"]);
  assert.equal(run.status, 0);
});

test("the CLI fails when a landed task still has pending rows", () => {
  const run = runChecker(["--upto", "99"]);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /Test manifest check FAILED:/);
  assert.match(run.stderr, /has landed but this row is still pending/);
});

test("the CLI exits 2 on a missing --upto value", () => {
  const run = runChecker(["--upto"]);
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Usage: node scripts\/check-test-manifest\.mjs/);
});

test("the CLI exits 2 when the manifest is missing", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "manifest-fixture-"));
  fixtureDirs.push(root);
  const run = runChecker(["--upto", "1"], { root });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /No manifest at /);
});

test("the CLI exits 2 without --upto when STATUS.md is missing", () => {
  const { root } = fixtureRoot({ manifest: `${HEADER}\n` });
  const run = runChecker([], { root });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /No STATUS\.md at /);
});

test("the CLI exits 2 without --upto when no task is done", () => {
  const { root } = fixtureRoot({ manifest: `${HEADER}\n`, status: "| Task | Title | State | Commit | Notes |\n" });
  const run = runChecker([], { root });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /No task is marked Done in STATUS\.md/);
});

test("the CLI reads the highest done task from STATUS.md", () => {
  const { root, bin } = fixtureRoot({
    manifest: `${HEADER}\npkg::test_one,2,ported,parsing::one,\n`,
    status: STATUS,
    nextest: JSON.stringify({ "rust-suites": { parsing: { testcases: { one: {} } } } }),
  });
  const run = runChecker([], { root, bin });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /1 owned by task <= 2, 1 ported/);
});

test("the CLI asks nextest for the ported row's test and fails when it is missing", () => {
  const { root, bin } = fixtureRoot({
    manifest: `${HEADER}\npkg::test_one,2,ported,parsing::one,\n`,
    nextest: JSON.stringify({ "rust-suites": { "flakeradar::parsing": { testcases: { other: {} } } } }),
  });
  const run = runChecker(["--upto", "2"], { root, bin });
  assert.equal(run.status, 1);
  assert.match(run.stderr, /rust_test "parsing::one" is not a test nextest knows/);
});

test("the CLI exits 2 when cargo nextest is not runnable", () => {
  const { root, bin } = fixtureRoot({ manifest: `${HEADER}\npkg::test_one,2,ported,parsing::one,\n` });
  // No shim: an empty PATH makes the spawn fail rather than finding a real cargo.
  const run = spawnSync(process.execPath, [SCRIPT, "--upto", "2", "--root", root], {
    encoding: "utf8",
    env: { ...process.env, PATH: bin },
  });
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Could not run cargo nextest/);
});
