import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkFloors,
  checkNoDecrease,
  parseArgs,
  parseFloors,
  readTotals,
  renderReport,
  worstFiles,
} from "./coverage-gate.mjs";

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), "coverage-gate.mjs");
const ROOT = "/repo";

function metric(covered, count) {
  return { covered, count, percent: count === 0 ? 0 : (covered / count) * 100 };
}

function fileEntry(filename, lines, branches) {
  return {
    filename,
    summary: {
      lines: metric(...lines),
      branches: metric(...branches),
      functions: metric(1, 1),
      regions: metric(1, 1),
    },
  };
}

const SUMMARY = {
  data: [
    {
      totals: {
        lines: metric(870, 1000),
        branches: metric(71, 100),
        functions: metric(77, 100),
        regions: metric(86, 100),
      },
      files: [
        fileEntry("/repo/crates/flakeradar-core/src/a.rs", [90, 100], [8, 10]),
        fileEntry("/repo/crates/flakeradar-core/src/b.rs", [50, 100], [0, 0]),
        fileEntry("/repo/crates/flakeradar-core/src/c.rs", [100, 100], [10, 10]),
        fileEntry("/repo/crates/flakeradar-core/src/d.rs", [100, 100], [3, 4]),
      ],
    },
  ],
};

/** A summary whose totals are the given percentages. */
function summaryWith(lines, branches) {
  return {
    data: [
      {
        totals: {
          lines: { covered: 0, count: 100, percent: lines },
          branches: { covered: 0, count: 100, percent: branches },
          functions: metric(1, 1),
          regions: metric(1, 1),
        },
        files: [],
      },
    ],
  };
}

const fixtureDirs = [];
process.on("exit", () => {
  for (const dir of fixtureDirs) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** Write `files` into a throwaway directory and return its path. */
function fixtureDir(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "coverage-gate-"));
  fixtureDirs.push(dir);
  for (const [name, contents] of Object.entries(files)) {
    fs.writeFileSync(path.join(dir, name), JSON.stringify(contents, null, 2));
  }
  return dir;
}

/** Run the gate with `args`, returning its exit status and streams. */
function runGate(args, env = {}) {
  const childEnv = { ...process.env, ...env };
  if (!("GITHUB_STEP_SUMMARY" in env)) {
    delete childEnv.GITHUB_STEP_SUMMARY;
  }
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", env: childEnv });
}

test("readTotals extracts every metric's covered, count and percent", () => {
  const totals = readTotals(SUMMARY);
  assert.deepEqual(totals.lines, { covered: 870, count: 1000, percent: 87 });
  assert.equal(totals.branches.percent, 71);
});

test("readTotals rejects a summary with no totals", () => {
  assert.throws(() => readTotals({ data: [] }), /totals/);
});

test("readTotals rejects a summary missing a metric", () => {
  const partial = { data: [{ totals: { lines: metric(1, 1) } }] };
  assert.throws(() => readTotals(partial), /branches/);
});

test("parseFloors accepts known metrics with percentages", () => {
  assert.deepEqual(parseFloors({ lines: 87, branches: 71.5 }), { lines: 87, branches: 71.5 });
});

for (const [name, input, message] of [
  ["null", null, /object/],
  ["an array", [87], /object/],
  ["an unknown metric", { statements: 80 }, /unknown/],
  ["a string", { lines: "87" }, /number/],
  ["a value above 100", { lines: 101 }, /0 to 100/],
  ["a negative value", { branches: -1 }, /0 to 100/],
]) {
  test(`parseFloors rejects ${name}`, () => {
    assert.throws(() => parseFloors(input), message);
  });
}

test("checkFloors passes when every total meets its floor exactly", () => {
  assert.deepEqual(checkFloors(readTotals(SUMMARY), { lines: 87, branches: 71 }), []);
});

test("checkFloors reports each metric below its floor", () => {
  const problems = checkFloors(readTotals(SUMMARY), { lines: 88, branches: 72 });
  assert.equal(problems.length, 2);
  assert.match(problems[0], /lines coverage 87\.00% is below the floor of 88%/);
  assert.match(problems[1], /branches/);
});

test("checkFloors ignores metrics without a floor", () => {
  assert.deepEqual(checkFloors(readTotals(SUMMARY), { lines: 50 }), []);
});

test("checkNoDecrease passes when lines and branches hold their baseline", () => {
  const totals = readTotals(SUMMARY);
  assert.deepEqual(checkNoDecrease(totals, readTotals(SUMMARY)), []);
});

test("checkNoDecrease compares at two decimals, so a smaller drift is not a fall", () => {
  const totals = readTotals(summaryWith(87.0, 71.0));
  const baseline = readTotals(summaryWith(87.004, 71.004));
  assert.deepEqual(checkNoDecrease(totals, baseline), []);
});

test("checkNoDecrease reports a one-hundredth fall in either metric", () => {
  const totals = readTotals(summaryWith(86.99, 70.99));
  const baseline = readTotals(summaryWith(87.0, 71.0));
  const problems = checkNoDecrease(totals, baseline);
  assert.equal(problems.length, 2);
  assert.match(problems[0], /lines coverage 86\.99% is below the baseline of 87\.00%/);
  assert.match(problems[1], /branches coverage 70\.99% is below the baseline of 71\.00%/);
});

test("checkNoDecrease reports only the metric that fell", () => {
  const totals = readTotals(summaryWith(87.0, 70.0));
  const baseline = readTotals(summaryWith(87.0, 71.0));
  const problems = checkNoDecrease(totals, baseline);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /^branches/);
});

test("worstFiles ranks by missed lines, then missed branches, relative to the root", () => {
  const worst = worstFiles(SUMMARY, ROOT);
  assert.deepEqual(
    worst.map((w) => w.file),
    [
      "crates/flakeradar-core/src/b.rs",
      "crates/flakeradar-core/src/a.rs",
      "crates/flakeradar-core/src/d.rs",
    ],
  );
});

test("worstFiles reports no branch percent for a file with no branches", () => {
  const b = worstFiles(SUMMARY, ROOT).find((w) => w.file.endsWith("b.rs"));
  assert.equal(b.branchPercent, null);
});

test("worstFiles respects the limit", () => {
  assert.equal(worstFiles(SUMMARY, ROOT, 1).length, 1);
});

test("renderReport lists every metric with its floor and the worst files", () => {
  const report = renderReport(readTotals(SUMMARY), { lines: 87 }, worstFiles(SUMMARY, ROOT));
  assert.match(report, /\| lines \| 870 \/ 1000 \| 87\.00 \| 87% \|/);
  assert.match(report, /\| branches \| 71 \/ 100 \| 71\.00 \| — \|/);
  assert.match(report, /\| `crates\/flakeradar-core\/src\/b\.rs` \| 50\.00 \| 50 \| — \| 0 \|/);
});

test("renderReport adds a baseline column when a baseline is given", () => {
  const report = renderReport(
    readTotals(SUMMARY),
    { lines: 87 },
    [],
    readTotals(summaryWith(88.5, 70)),
  );
  assert.match(report, /\| Metric \| Covered \| % \| Floor \| Baseline \|/);
  assert.match(report, /\| lines \| 870 \/ 1000 \| 87\.00 \| 87% \| 88\.50% \|/);
});

test("renderReport omits the file table when nothing is missed", () => {
  const report = renderReport(readTotals(SUMMARY), {}, []);
  assert.doesNotMatch(report, /Most missed/);
});

test("parseArgs splits the summary, the baseline and the floor", () => {
  assert.deepEqual(parseArgs([]), { positional: [], baselinePath: null });
  assert.deepEqual(parseArgs(["s.json", "f.json"]), {
    positional: ["s.json", "f.json"],
    baselinePath: null,
  });
  assert.deepEqual(parseArgs(["s.json", "--baseline", "b.json"]), {
    positional: ["s.json"],
    baselinePath: "b.json",
  });
});

test("parseArgs rejects a baseline flag with no path", () => {
  assert.throws(() => parseArgs(["s.json", "--baseline"]), /needs a file path/);
});

test("parseArgs rejects an unknown option", () => {
  assert.throws(() => parseArgs(["--base", "b.json"]), /unknown option --base/);
});

test("the gate passes, and writes the report to the job summary", () => {
  const dir = fixtureDir({
    "summary.json": SUMMARY,
    "floor.json": { lines: 87, branches: 71 },
    "baseline.json": SUMMARY,
  });
  const stepSummary = path.join(dir, "step-summary.md");
  const run = runGate(
    [path.join(dir, "summary.json"), path.join(dir, "floor.json")],
    { GITHUB_STEP_SUMMARY: stepSummary },
  );
  assert.equal(run.status, 0);
  assert.match(run.stdout, /## Rust coverage/);
  assert.match(run.stdout, /Coverage gate passed\./);
  assert.match(fs.readFileSync(stepSummary, "utf8"), /## Rust coverage/);
});

test("the gate fails when a total is below its floor", () => {
  const dir = fixtureDir({ "summary.json": SUMMARY, "floor.json": { lines: 88 } });
  const run = runGate([path.join(dir, "summary.json"), path.join(dir, "floor.json")]);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /Coverage gate FAILED:/);
  assert.match(run.stderr, /below the floor of 88%/);
});

test("the gate fails when a total falls below the baseline", () => {
  const dir = fixtureDir({
    "summary.json": summaryWith(87, 71),
    "floor.json": { lines: 0, branches: 0 },
    "baseline.json": summaryWith(91, 71),
  });
  const run = runGate([
    path.join(dir, "summary.json"),
    "--baseline",
    path.join(dir, "baseline.json"),
    path.join(dir, "floor.json"),
  ]);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /below the baseline of 91\.00%/);
});

test("the gate exits 2 with usage when it is given no summary", () => {
  const run = runGate([]);
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Usage: node scripts\/coverage-gate\.mjs/);
});

test("the gate exits 2 on an unreadable summary", () => {
  const run = runGate([path.join(os.tmpdir(), "no-such-coverage-summary.json")]);
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Coverage gate input error:/);
});
