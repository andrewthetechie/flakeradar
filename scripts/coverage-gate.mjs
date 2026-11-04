#!/usr/bin/env node
/**
 * Rust coverage gate.
 *
 * Reads the JSON summary that `cargo llvm-cov report --json --summary-only`
 * writes, compares the line and branch totals against the floors in
 * `coverage-floor.json` at the repo root and, when a baseline is given, against
 * that baseline too, then prints a Markdown report. CI appends the report to the
 * job summary.
 *
 * cargo-llvm-cov has `--fail-under-lines` but no branch equivalent, so both
 * floors are checked here, in one place.
 *
 *   node scripts/coverage-gate.mjs <summary.json> [--baseline <baseline.json>] [floor.json]
 *
 * The baseline is the `rust-coverage` artifact that a push to `main` uploads,
 * which is the same summary format. Lines or branches below it fail the gate at
 * two decimals, with no tolerance. With no baseline, only the floors apply.
 *
 * Exits 1 when a total is below its floor or its baseline, 2 on bad input.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_FLOOR_PATH = path.join(REPO_ROOT, "coverage-floor.json");

/** Metrics the gate knows how to check, in report order. */
export const METRICS = ["lines", "branches", "functions", "regions"];

/** Metrics that must never fall below the baseline. */
export const NO_DECREASE_METRICS = ["lines", "branches"];

/** Percentages are compared and printed at this many decimals. */
const DECIMALS = 2;

/** How many files the report lists, ranked by missed lines. */
const WORST_FILE_COUNT = 15;

// ── Pure helpers (unit-tested) ────────────────────────────────────────────

/**
 * Pull `{ covered, count, percent }` for each metric out of an llvm-cov JSON
 * export. Throws when the export has no totals.
 */
export function readTotals(summary) {
  const totals = summary?.data?.[0]?.totals;
  if (totals == null) {
    throw new Error("coverage summary has no data[0].totals");
  }
  const result = {};
  for (const metric of METRICS) {
    const t = totals[metric];
    if (t == null) {
      throw new Error(`coverage summary has no totals.${metric}`);
    }
    result[metric] = { covered: t.covered, count: t.count, percent: t.percent };
  }
  return result;
}

/**
 * Validate a floor file's contents: an object mapping known metrics to a
 * percentage between 0 and 100. Returns the floors; throws on anything else.
 */
export function parseFloors(floors) {
  if (floors == null || typeof floors !== "object" || Array.isArray(floors)) {
    throw new Error("coverage floor must be a JSON object");
  }
  const result = {};
  for (const [metric, value] of Object.entries(floors)) {
    if (!METRICS.includes(metric)) {
      throw new Error(`unknown coverage metric in floor: ${metric}`);
    }
    if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 100) {
      throw new Error(`coverage floor for ${metric} must be a number from 0 to 100`);
    }
    result[metric] = value;
  }
  return result;
}

/** Round a percentage to the precision the report prints. */
export function atTwoDecimals(percent) {
  return Number(percent.toFixed(DECIMALS));
}

/** Return one message per metric whose total is below its floor. */
export function checkFloors(totals, floors) {
  const problems = [];
  for (const [metric, floor] of Object.entries(floors)) {
    const actual = totals[metric].percent;
    if (actual < floor) {
      problems.push(`${metric} coverage ${actual.toFixed(DECIMALS)}% is below the floor of ${floor}%`);
    }
  }
  return problems;
}

/**
 * Return one message per metric that fell below the baseline, compared at two
 * decimals. A total that moved by less than 0.005 counts as unchanged, so float
 * noise in the report cannot fail the gate.
 */
export function checkNoDecrease(totals, baseline) {
  const problems = [];
  for (const metric of NO_DECREASE_METRICS) {
    const actual = atTwoDecimals(totals[metric].percent);
    const was = atTwoDecimals(baseline[metric].percent);
    if (actual < was) {
      problems.push(
        `${metric} coverage ${actual.toFixed(DECIMALS)}% is below the baseline of ${was.toFixed(DECIMALS)}%`,
      );
    }
  }
  return problems;
}

/**
 * Rank files by missed lines, worst first, with paths made relative to `root`.
 * Files with nothing missed are left out.
 */
export function worstFiles(summary, root, limit = WORST_FILE_COUNT) {
  const files = summary?.data?.[0]?.files ?? [];
  const prefix = root.endsWith(path.sep) ? root : root + path.sep;
  return files
    .map((f) => {
      const s = f.summary;
      return {
        file: f.filename.startsWith(prefix) ? f.filename.slice(prefix.length) : f.filename,
        linePercent: s.lines.percent,
        missedLines: s.lines.count - s.lines.covered,
        branchPercent: s.branches.count === 0 ? null : s.branches.percent,
        missedBranches: s.branches.count - s.branches.covered,
      };
    })
    .filter((f) => f.missedLines > 0 || f.missedBranches > 0)
    .sort((a, b) => b.missedLines - a.missedLines || b.missedBranches - a.missedBranches)
    .slice(0, limit);
}

/**
 * Render the totals, floors, baseline and worst files as a Markdown report.
 * The baseline column appears only when `baseline` is given.
 */
export function renderReport(totals, floors, worst, baseline = null) {
  const header = baseline == null ? "| Metric | Covered | % | Floor |" : "| Metric | Covered | % | Floor | Baseline |";
  const rule = baseline == null ? "|---|---|---|---|" : "|---|---|---|---|---|";
  const lines = ["## Rust coverage", "", header, rule];
  for (const metric of METRICS) {
    const t = totals[metric];
    const floor = floors[metric] == null ? "—" : `${floors[metric]}%`;
    const cells = `| ${metric} | ${t.covered} / ${t.count} | ${t.percent.toFixed(DECIMALS)} | ${floor} |`;
    if (baseline == null) {
      lines.push(cells);
    } else {
      const was = baseline[metric] == null ? "—" : `${baseline[metric].percent.toFixed(DECIMALS)}%`;
      lines.push(`${cells} ${was} |`);
    }
  }
  if (worst.length > 0) {
    lines.push(
      "",
      `### Most missed lines (top ${worst.length})`,
      "",
      "| File | Line % | Missed lines | Branch % | Missed branches |",
      "|---|---|---|---|---|",
    );
    for (const w of worst) {
      const branch = w.branchPercent == null ? "—" : w.branchPercent.toFixed(DECIMALS);
      lines.push(
        `| \`${w.file}\` | ${w.linePercent.toFixed(DECIMALS)} | ${w.missedLines} | ${branch} | ${w.missedBranches} |`,
      );
    }
  }
  return lines.join("\n") + "\n";
}

// ── CLI ───────────────────────────────────────────────────────────────────

const USAGE =
  "Usage: node scripts/coverage-gate.mjs <summary.json> [--baseline <baseline.json>] [floor.json]";

/**
 * Split the arguments into positional paths and the `--baseline` path. Throws on
 * a flag it does not know, so a typo cannot silently drop the no-decrease check.
 */
export function parseArgs(args) {
  const positional = [];
  let baselinePath = null;
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === "--baseline") {
      const value = args[i + 1];
      if (value == null) {
        throw new Error("--baseline needs a file path");
      }
      baselinePath = value;
      i += 1;
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown option ${arg}`);
    } else {
      positional.push(arg);
    }
  }
  return { positional, baselinePath };
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function main(args) {
  let summary;
  let totals;
  let floors;
  let baseline = null;
  try {
    const { positional, baselinePath } = parseArgs(args);
    const [summaryPath, floorPath = DEFAULT_FLOOR_PATH] = positional;
    if (summaryPath == null) {
      console.error(USAGE);
      process.exit(2);
    }
    summary = readJson(summaryPath);
    totals = readTotals(summary);
    floors = parseFloors(readJson(floorPath));
    if (baselinePath != null) {
      baseline = readTotals(readJson(baselinePath));
    }
  } catch (e) {
    console.error(`Coverage gate input error: ${e.message}`);
    process.exit(2);
  }

  const report = renderReport(totals, floors, worstFiles(summary, REPO_ROOT), baseline);
  console.log(report);
  if (process.env.GITHUB_STEP_SUMMARY) {
    fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, report);
  }

  const problems = checkFloors(totals, floors);
  if (baseline != null) {
    problems.push(...checkNoDecrease(totals, baseline));
  }
  if (problems.length > 0) {
    console.error("Coverage gate FAILED:");
    for (const p of problems) {
      console.error(`  - ${p}`);
    }
    process.exit(1);
  }
  console.log("Coverage gate passed.");
}

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main(process.argv.slice(2));
}
