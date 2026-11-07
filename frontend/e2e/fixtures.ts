// Fixture loader for the `ui` Playwright project. Reads the JSON fixtures as
// plain data so a mock answers exactly the bytes a real server would, and the
// `e2e` drift test can compare the live server's top-level field names against
// the same files.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

/** Read one fixture and parse it as JSON. */
export function readFixture<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, `${name}.json`), "utf8")) as T;
}

/** Top-level field names of a fixture's representation: an object's keys, or
 *  a single array element's keys (an array endpoint's "fields"). */
export function fixtureFields(name: string): string[] {
  const data = readFixture<unknown>(name) as Record<string, unknown> | unknown[];
  const sample = Array.isArray(data) ? (data[0] ?? {}) : data;
  return Object.keys(sample as Record<string, unknown>).sort();
}
