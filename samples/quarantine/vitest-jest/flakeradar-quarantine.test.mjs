// The Vitest and Jest quarantine helper. These cases check the pure part: the
// pattern built from this sample's vitest/quarantine.json fixture, which is the
// RegExp documented in the comment block above buildTestNamePattern and asserted
// here character for character. The runner cases in this file drive the real
// Vitest and Jest runners against a local server.
//
// Run from the repository root, with no install needed for these cases:
//
//   node --test samples/quarantine/vitest-jest/flakeradar-quarantine.test.mjs
//
// or in Docker with samples/quarantine/test.sh vitest-jest, which installs the
// pinned runners first.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

import { buildTestNamePattern } from "./flakeradar-quarantine.mjs";

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
