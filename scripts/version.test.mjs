import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkVersions,
  parseWorkspaceVersion,
  readAllVersions,
  validateStableVersion,
} from "./version.mjs";

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), "version.mjs");
const REPO_ROOT = path.join(path.dirname(SCRIPT), "..");

const CARGO_TOML = `[workspace]
members = ["crates/flakeradar"]
resolver = "3"

[workspace.package]
version = "2.0.0"
edition = "2024"
license = "Apache-2.0"

[workspace.dependencies]
anyhow = { version = "1", default-features = false }
`;

const fixtureDirs = [];
process.on("exit", () => {
  for (const dir of fixtureDirs) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** Build a throwaway repo-shaped fixture tree and return its root. */
function fixtureRoot(versions = {}) {
  const { cargo = "2.0.0", pkg = "2.0.0", lock = "2.0.0", lockPackages = "2.0.0" } = versions;
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "version-fixture-"));
  fixtureDirs.push(root);
  fs.mkdirSync(path.join(root, "frontend"));
  fs.writeFileSync(
    path.join(root, "Cargo.toml"),
    CARGO_TOML.replace('version = "2.0.0"', `version = "${cargo}"`),
  );
  fs.writeFileSync(
    path.join(root, "frontend", "package.json"),
    `${JSON.stringify({ name: "flakeradar-frontend", version: pkg }, null, 2)}\n`,
  );
  fs.writeFileSync(
    path.join(root, "frontend", "package-lock.json"),
    `${JSON.stringify({ version: lock, packages: { "": { version: lockPackages } } }, null, 2)}\n`,
  );
  return root;
}

test("parseWorkspaceVersion reads the [workspace.package] version", () => {
  assert.equal(parseWorkspaceVersion(CARGO_TOML), "2.0.0");
});

test("parseWorkspaceVersion ignores dependency and member versions", () => {
  const content = `${CARGO_TOML}\n[package]\nname = "x"\nversion = "9.9.9"\n`;
  assert.equal(parseWorkspaceVersion(content), "2.0.0");
});

test("parseWorkspaceVersion returns null without a [workspace.package] table", () => {
  assert.equal(parseWorkspaceVersion('[package]\nname = "x"\nversion = "1.0.0"\n'), null);
});

test("parseWorkspaceVersion returns null without a version key", () => {
  assert.equal(parseWorkspaceVersion("[workspace.package]\nedition = \"2024\"\n"), null);
});

test("validateStableVersion accepts MAJOR.MINOR.PATCH", () => {
  assert.equal(validateStableVersion("0.1.0"), true);
  assert.equal(validateStableVersion("1.2.3"), true);
});

test("validateStableVersion rejects a leading v, short forms and prereleases", () => {
  assert.equal(validateStableVersion("v1.2.3"), false);
  assert.equal(validateStableVersion("1.2"), false);
  assert.equal(validateStableVersion("1.2.3-beta"), false);
  assert.equal(validateStableVersion("1.2.3.4"), false);
});

test("readAllVersions reads every source", () => {
  const versions = readAllVersions(fixtureRoot());
  assert.deepEqual(
    versions.map((v) => v.version),
    ["2.0.0", "2.0.0", "2.0.0", "2.0.0"],
  );
});

test("readAllVersions returns null when the lock has no packages entry", () => {
  const root = fixtureRoot();
  fs.writeFileSync(path.join(root, "frontend", "package-lock.json"), '{"version": "2.0.0"}\n');
  const versions = readAllVersions(root);
  assert.equal(versions[3].version, undefined);
});

test("checkVersions passes when every source agrees", () => {
  assert.deepEqual(
    checkVersions([
      { label: "a", version: "0.1.0" },
      { label: "b", version: "0.1.0" },
    ]),
    [],
  );
});

test("checkVersions flags drift across sources by label", () => {
  const problems = checkVersions([
    { label: "a", version: "0.1.0" },
    { label: "b", version: "0.2.0" },
  ]);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /versions differ across sources: a=0\.1\.0, b=0\.2\.0/);
});

test("checkVersions flags a non-stable version", () => {
  const problems = checkVersions([{ label: "a", version: "v0.1.0" }]);
  assert.equal(problems.length, 1);
  assert.match(problems[0], /a has non-stable version "v0\.1\.0"/);
});

test("checkVersions enforces the expected version when given", () => {
  const versions = [{ label: "a", version: "0.1.0" }];
  assert.deepEqual(checkVersions(versions, "0.1.0"), []);
  assert.match(checkVersions(versions, "0.2.0")[0], /a has "0\.1\.0", expected "0\.2\.0"/);
});

test("the committed manifests agree on one version", () => {
  assert.deepEqual(checkVersions(readAllVersions(REPO_ROOT)), []);
});

/** Run the CLI, returning its exit status and streams. */
function runVersion(args) {
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8" });
}

test("the CLI check passes against the committed tree", () => {
  const run = runVersion(["check"]);
  assert.equal(run.status, 0);
  assert.match(run.stdout, /Version check passed\./);
});

test("the CLI check passes against the current workspace version", () => {
  const [workspace] = readAllVersions(REPO_ROOT);
  const run = runVersion(["check", workspace.version]);
  assert.equal(run.status, 0);
});

test("the CLI check fails against a version the tree does not have", () => {
  const run = runVersion(["check", "1234.0.0"]);
  assert.equal(run.status, 1);
  assert.match(run.stderr, /Version check FAILED:/);
});

test("the CLI refuses a non-stable expected version", () => {
  const run = runVersion(["check", "v2.0.0"]);
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Refusing to check against "v2\.0\.0"/);
});

test("the CLI exits 2 with usage on an unknown command", () => {
  const run = runVersion(["bump", "3.0.0"]);
  assert.equal(run.status, 2);
  assert.match(run.stderr, /Usage: node scripts\/version\.mjs check \[version\]/);
});
