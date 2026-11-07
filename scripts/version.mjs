#!/usr/bin/env node
/**
 * Single place that knows every version-bearing file.
 *
 * The release version lives in two files that nothing keeps in step:
 *
 *   - Cargo.toml  (`[workspace.package] version`, which every crate inherits)
 *   - frontend/package.json
 *   - frontend/package-lock.json  (root `version` and `packages[""].version`)
 *
 * The workspace manifest is the source of truth; this script only keeps the
 * frontend equal to it.
 *
 * Commands:
 *
 *   node scripts/version.mjs check [expected]   — fail if any source drifts
 *
 * CI runs `check` with no argument on every push and PR. The release workflow
 * runs `check` against the tag before building.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Stable MAJOR.MINOR.PATCH with no leading "v". Prereleases are out of scope. */
const STABLE = /^\d+\.\d+\.\d+$/;

// ── Pure parsers (unit-tested) ────────────────────────────────────────────

/** Parse the `version` in `[workspace.package]` from a Cargo.toml string. */
export function parseWorkspaceVersion(content) {
  const pkg = content.match(/\[workspace\.package\]([\s\S]*?)(?=\n\[|$)/);
  if (!pkg) return null;
  const m = pkg[1].match(/version\s*=\s*"([^"]+)"/);
  return m ? m[1] : null;
}

export function validateStableVersion(version) {
  return STABLE.test(version);
}

// ── File-level reads (unit-tested against a fixture root) ────────────────

/**
 * Read every source version under `root`. Returns an array of
 * `{ label, version }` in a stable order.
 */
export function readAllVersions(root = REPO_ROOT) {
  const workspaceToml = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const pkg = JSON.parse(fs.readFileSync(path.join(root, "frontend", "package.json"), "utf8"));
  const lock = JSON.parse(fs.readFileSync(path.join(root, "frontend", "package-lock.json"), "utf8"));

  return [
    { label: "Cargo.toml", version: parseWorkspaceVersion(workspaceToml) },
    { label: "frontend/package.json", version: pkg.version },
    { label: "frontend/package-lock.json (root)", version: lock.version },
    { label: "frontend/package-lock.json (packages)", version: lock.packages?.[""]?.version },
  ];
}

/**
 * Return an array of human-readable problems, or empty when consistent.
 * When `expected` is given, every source must equal it exactly.
 */
export function checkVersions(versions, expected) {
  const problems = [];

  const labels = versions.map((v) => `${v.label}=${v.version}`);
  const unique = new Set(versions.map((v) => v.version));
  if (unique.size !== 1) {
    problems.push(`versions differ across sources: ${labels.join(", ")}`);
  }

  for (const { label, version } of versions) {
    if (!validateStableVersion(version)) {
      problems.push(
        `${label} has non-stable version "${version}" (expected MAJOR.MINOR.PATCH, no leading "v")`,
      );
    }
  }

  if (expected != null) {
    for (const { label, version } of versions) {
      if (version !== expected) {
        problems.push(`${label} has "${version}", expected "${expected}"`);
      }
    }
  }

  return problems;
}

// ── CLI ───────────────────────────────────────────────────────────────────

function main(args) {
  const [command, value] = args;

  if (command !== "check") {
    console.error("Usage: node scripts/version.mjs check [version]");
    process.exit(2);
  }

  const expected = value;
  if (expected != null && !validateStableVersion(expected)) {
    console.error(`Refusing to check against "${expected}": expected stable MAJOR.MINOR.PATCH (no leading "v").`);
    process.exit(2);
  }

  const versions = readAllVersions(REPO_ROOT);
  for (const { label, version } of versions) {
    console.log(`${label}: ${version}`);
  }
  const problems = checkVersions(versions, expected);
  if (problems.length > 0) {
    console.error("Version check FAILED:");
    for (const p of problems) {
      console.error(`  - ${p}`);
    }
    process.exit(1);
  }
  console.log("Version check passed.");
}

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main(process.argv.slice(2));
}
