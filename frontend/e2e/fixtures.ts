// Fixture loader for the `ui` Playwright project. Reads the JSON fixtures as
// plain data so a mock answers exactly the bytes a real server would, and the
// `e2e` drift test can compare the live server's field names against the same
// files with the same function.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

/** Read one fixture and parse it as JSON. */
export function readFixture<T = unknown>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, `${name}.json`), "utf8")) as T;
}

/**
 * The field names an API answer carries, as the dashboard reads them: the keys
 * of the object it got — or of the first element, for a route that answers with
 * a bare list — plus the keys of the rows inside `items`, which is where the
 * leaderboard's and the Fabro contract's fields live.
 */
export function answerFields(answer: unknown): string[] {
  const row = Array.isArray(answer) ? answer[0] : answer;
  if (typeof row !== "object" || row === null) return [];
  const names = Object.keys(row).sort();
  const items = (row as { items?: unknown }).items;
  if (Array.isArray(items) && items.length > 0 && typeof items[0] === "object") {
    names.push(
      `items[].${Object.keys(items[0] as object)
        .sort()
        .join(",")}`,
    );
  }
  return names;
}

/** `answerFields` of one fixture file. */
export function fixtureFields(name: string): string[] {
  return answerFields(readFixture(name));
}
