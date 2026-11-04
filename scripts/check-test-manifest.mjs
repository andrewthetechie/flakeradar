#!/usr/bin/env node
/**
 * Keep `docs/plans/rust-rewrite/test-manifest.csv` honest.
 *
 * The manifest lists every pytest function in the frozen Python backend and the
 * Rust rewrite task that ports it. This script fails the build when a task that
 * has landed still carries `pending` rows, when a `ported` row names a Rust test
 * that does not exist, or when an `na` or `differs` row does not say why.
 *
 * Usage:
 *
 *   node scripts/check-test-manifest.mjs [--upto NN] [--root DIR]
 *
 * `--upto NN` is the highest task whose rows must be done. Without it the last
 * task marked `Done` in STATUS.md is used, so CI needs no hand-maintained
 * number. `--root DIR` reads the two files from DIR instead of this repo; the
 * tests use it to run against fixtures.
 */

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const MANIFEST_PATH = "docs/plans/rust-rewrite/test-manifest.csv";
const STATUS_PATH = "docs/plans/rust-rewrite/STATUS.md";
const COLUMNS = ["pytest_id", "task", "status", "rust_test", "reason"];
const STATUSES = new Set(["pending", "ported", "na", "differs"]);

// ── Parsing (unit-tested) ─────────────────────────────────────────────────

/** Split CSV text into rows of fields. Handles quoted fields and doubled quotes. */
export function parseCsv(text) {
  const rows = [];
  let row = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 1;
        } else {
          quoted = false;
        }
      } else {
        field += ch;
      }
    } else if (ch === '"') {
      quoted = true;
    } else if (ch === ",") {
      row.push(field);
      field = "";
    } else if (ch === "\n") {
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else if (ch !== "\r") {
      field += ch;
    }
  }
  if (field !== "" || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows.filter((r) => !(r.length === 1 && r[0] === ""));
}

/**
 * Parse the manifest text into `{ rows, problems }`.
 *
 * Rows carry the five columns as strings plus the 1-based line number. A row
 * whose column count or status is wrong is a problem, not a row.
 */
export function parseManifest(text) {
  const problems = [];
  const table = parseCsv(text);
  if (table.length === 0) {
    return { rows: [], problems: ["the manifest is empty"] };
  }
  const header = table[0];
  if (header.join(",") !== COLUMNS.join(",")) {
    problems.push(`header is "${header.join(",")}", expected "${COLUMNS.join(",")}"`);
  }
  const rows = [];
  for (let i = 1; i < table.length; i += 1) {
    const fields = table[i];
    const line = i + 1;
    if (fields.length !== COLUMNS.length) {
      problems.push(`line ${line}: ${fields.length} columns, expected ${COLUMNS.length}`);
      continue;
    }
    const [pytestId, task, status, rustTest, reason] = fields;
    if (!STATUSES.has(status)) {
      problems.push(`line ${line}: status "${status}" is not one of ${[...STATUSES].join(", ")}`);
      continue;
    }
    if (!/^\d+$/.test(task)) {
      problems.push(`line ${line}: task "${task}" is not a number`);
      continue;
    }
    rows.push({ line, pytestId, task: Number(task), status, rustTest, reason });
  }
  return { rows, problems };
}

/**
 * The highest task number marked `Done` in STATUS.md, or null when the table
 * has no done task.
 */
export function lastDoneTask(statusText) {
  let last = null;
  for (const line of statusText.split("\n")) {
    const cells = line.split("|").map((c) => c.trim());
    // | 03 | Title | Done | sha | notes |  → ["", "03", "Title", "Done", "sha", "notes", ""]
    if (cells.length < 5 || !/^\d+$/.test(cells[1])) continue;
    if (cells[3] !== "Done") continue;
    const task = Number(cells[1]);
    if (last === null || task > last) last = task;
  }
  return last;
}

/**
 * Problems for the rows of task <= `upto`, given the Rust test names nextest
 * knows. With `knownTests` null the `ported` rows are not checked, because
 * nextest was not asked.
 */
export function checkManifest(rows, upto, knownTests) {
  const problems = [];
  for (const row of rows) {
    if (row.task > upto) continue;
    if (row.status === "pending") {
      problems.push(`${row.pytestId}: task ${row.task} has landed but this row is still pending`);
    } else if (row.status === "na") {
      if (row.reason.trim() === "") {
        problems.push(`${row.pytestId}: marked na without a reason`);
      }
    } else if (row.status === "differs") {
      // The behaviour intentionally differs from Python's: the reason names the
      // differences.md row that covers it.
      if (row.reason.trim() === "") {
        problems.push(`${row.pytestId}: marked differs without a reason`);
      }
    } else if (row.status === "ported") {
      if (row.rustTest.trim() === "") {
        problems.push(`${row.pytestId}: marked ported without a rust_test`);
      } else if (knownTests !== null && !knownTests.has(row.rustTest)) {
        problems.push(`${row.pytestId}: rust_test "${row.rustTest}" is not a test nextest knows`);
      }
    }
  }
  return problems;
}

/** Every test name in a `cargo nextest list --message-format json` document. */
export function parseNextestList(jsonText) {
  const names = new Set();
  let doc;
  try {
    doc = JSON.parse(jsonText);
  } catch {
    return names;
  }
  for (const [suiteId, suite] of Object.entries(doc["rust-suites"] ?? {})) {
    for (const name of Object.keys(suite.testcases ?? {})) {
      names.add(name);
      names.add(`${suiteId}::${name}`);
    }
  }
  return names;
}

// ── CLI ───────────────────────────────────────────────────────────────────

function parseArgs(args) {
  const options = { upto: null, root: REPO_ROOT };
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === "--upto") {
      const value = args[i + 1];
      if (value === undefined || !/^\d+$/.test(value)) {
        console.error("Usage: node scripts/check-test-manifest.mjs [--upto NN] [--root DIR]");
        process.exit(2);
      }
      options.upto = Number(value);
      i += 1;
    } else if (arg === "--root") {
      const value = args[i + 1];
      if (value === undefined) {
        console.error("Usage: node scripts/check-test-manifest.mjs [--upto NN] [--root DIR]");
        process.exit(2);
      }
      options.root = path.resolve(value);
      i += 1;
    } else {
      console.error(`Unknown argument "${arg}".`);
      console.error("Usage: node scripts/check-test-manifest.mjs [--upto NN] [--root DIR]");
      process.exit(2);
    }
  }
  return options;
}

/** Ask nextest for every test name in the workspace. */
function knownRustTests(root) {
  const run = spawnSync(
    "cargo",
    ["nextest", "list", "--all-features", "--profile", "ci", "--message-format", "json"],
    { cwd: root, encoding: "utf8" },
  );
  if (run.error) {
    console.error(`Could not run cargo nextest: ${run.error.message}`);
    process.exit(2);
  }
  if (run.status !== 0) {
    console.error("cargo nextest list failed:");
    console.error(run.stderr.trim());
    process.exit(2);
  }
  return parseNextestList(run.stdout);
}

function main(args) {
  const { upto: explicitUpto, root } = parseArgs(args);

  const manifestFile = path.join(root, MANIFEST_PATH);
  const statusFile = path.join(root, STATUS_PATH);
  if (!fs.existsSync(manifestFile)) {
    console.error(`No manifest at ${manifestFile}.`);
    process.exit(2);
  }

  const { rows, problems } = parseManifest(fs.readFileSync(manifestFile, "utf8"));

  let upto = explicitUpto;
  if (upto === null) {
    if (!fs.existsSync(statusFile)) {
      console.error(`No STATUS.md at ${statusFile}; pass --upto NN instead.`);
      process.exit(2);
    }
    upto = lastDoneTask(fs.readFileSync(statusFile, "utf8"));
    if (upto === null) {
      console.error("No task is marked Done in STATUS.md; pass --upto NN instead.");
      process.exit(2);
    }
  }

  const inScope = rows.filter((row) => row.task <= upto);
  const ported = inScope.some((row) => row.status === "ported");
  const knownTests = ported ? knownRustTests(root) : null;

  problems.push(...checkManifest(rows, upto, knownTests));

  console.log(
    `${rows.length} rows, ${inScope.length} owned by task <= ${upto}, ` +
      `${inScope.filter((r) => r.status === "ported").length} ported, ` +
      `${inScope.filter((r) => r.status === "na").length} na.`,
  );
  if (problems.length > 0) {
    console.error("Test manifest check FAILED:");
    for (const problem of problems) {
      console.error(`  - ${problem}`);
    }
    process.exit(1);
  }
  console.log("Test manifest check passed.");
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main(process.argv.slice(2));
}
