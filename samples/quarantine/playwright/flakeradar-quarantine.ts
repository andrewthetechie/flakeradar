// The Playwright quarantine helper, documented at ../README.md.
//
// Copy this file next to your playwright.config.ts, import quarantineGrepInvert
// and use it as grepInvert, as shown above buildGrepInvert. It needs Node 18 or
// later, and nothing else: Playwright reads grepInvert while it loads the
// config and never awaits a Promise default export, so the helper cannot fetch
// there. It runs the request in a `node -e` child and waits for it with
// execFileSync, which is synchronous, and keeps the answer in
// FLAKERADAR_QUARANTINE_CACHE so that the worker processes, which inherit the
// environment, read it from there instead of fetching again. A list too long
// for one environment variable is written to a file instead, and only the
// path of that file goes into the environment the workers inherit: a variable
// of that length is refused by the operating system at the next process start,
// which would stop the run instead of skipping the tests, and a worker that has
// to ask for itself may hear something else, or nothing, and run what the first
// answer quarantined.
//
// The environment is FLAKERADAR_URL, FLAKERADAR_TOKEN, FLAKERADAR_REPO
// (falling back to GITHUB_REPOSITORY) and FLAKERADAR_PROJECT; an empty variable
// counts as unset. When the URL or the repo is unset nothing is fetched, nothing
// is cached and nothing is skipped. On any fetch, status or body problem one
// warning line goes to stderr, the cache is written as an empty list and nothing
// is skipped either: this helper never fails a run because of FlakeRadar.
//
// The names below are the ones the JUnit reporter writes with
// includeProjectInTestName left at its default, which is what FlakeRadar stores.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

/**
 * A quarantined test, as `GET /api/quarantine` answers it. Only `classname` and
 * `name` are used here; the other keys are what the endpoint writes, listed in
 * ../README.md.
 */
export interface QuarantineItem {
  /** The file suite's title: the spec path relative to the config's rootDir. */
  classname: string;
  /** The test title, with the describes above it joined by " › " when the
   *  runner's reporter writes them into the name. */
  name: string;
}

/**
 * Escapes the characters that would otherwise carry a regexp meaning, so a test
 * named `adds an item (v2)` still matches that name and nothing else.
 */
function esc(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Builds the RegExp that skips every quarantined test, or undefined when there
// is nothing to skip.
//
// Playwright matches grepInvert against the whole title path: the root suite's
// empty title, the project, the file, every describe and the test title, joined
// with " " and followed by the tags. FlakeRadar stores what the JUnit reporter
// writes for the same test: the classname is the file's title, and the name is
// the describes and the test title joined by " › ", so splitting the name on
// that separator gives back the titles that follow the classname in the grep
// title. One alternative is therefore `(?:^| )`, which puts the classname on a
// title boundary so that it cannot match the tail of a longer one, the escaped
// classname, a space, the escaped name parts joined with " ", and `(?=$| @)`,
// which asks for the end of the titles or the tags: `adds an item` skips a test
// tagged @slow, but not `adds an item twice`.
//
// Example input, this sample's quarantine.json, whose two items are the tests
// the JUnit reporter reports for example/cart.spec.ts:
//
//   [
//     {"suite": "cart.spec.ts", "classname": "cart.spec.ts",
//      "name": "cart › adds an item", "fingerprint": "0000000000000000000000000000000000000001",
//      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
//      "reason": "flaky in CI", "expires_at": "2026-12-01T12:00:00.000000Z"},
//     {"suite": "cart.spec.ts", "classname": "cart.spec.ts",
//      "name": "checkout total", "fingerprint": "0000000000000000000000000000000000000002",
//      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
//      "reason": null, "expires_at": null}
//   ]
//
// returns a RegExp whose source is exactly:
//
//   (?:^| )cart\.spec\.ts cart adds an item(?=$| @)|(?:^| )cart\.spec\.ts checkout total(?=$| @)
//
// Usage in playwright.config.ts:
//
//   import { defineConfig } from "@playwright/test";
//   import { quarantineGrepInvert } from "../flakeradar-quarantine.ts";
//
//   export default defineConfig({
//     testDir: ".",
//     projects: [{ name: "unit", grepInvert: quarantineGrepInvert() }],
//   });
//
// grepInvert also takes an array of patterns, so a team that has its own can
// combine the two. A call that skipped nothing returns undefined, and an
// undefined entry is not a pattern, so filter the list:
//
//   grepInvert: [teamOwnGrepInvert, quarantineGrepInvert()].filter(Boolean),
//
// @param items the quarantine list, in list order
// @returns undefined when the list is empty, otherwise the pattern
export function buildGrepInvert(items: QuarantineItem[]): RegExp | undefined {
  if (items.length === 0) {
    return undefined;
  }
  const alt = (item: QuarantineItem): string => {
    const titles = item.name.split(" \u203a ").map(esc).join(" ");
    return "(?:^| )" + esc(item.classname) + " " + titles + "(?=$| @)";
  };
  return new RegExp(items.map(alt).join("|"));
}

/** The value of an environment variable, or "" when it is unset or empty. */
function setting(name: string): string {
  return process.env[name] || "";
}

/** The variable that carries the cache to the processes this one spawns. */
const CACHE_ENV = "FLAKERADAR_QUARANTINE_CACHE";

/**
 * The variable that carries the path of a cache too long for one environment
 * string: a path is short whatever list it points at.
 */
const CACHE_FILE_ENV = "FLAKERADAR_QUARANTINE_CACHE_FILE";

/**
 * The longest cache this writes into FLAKERADAR_QUARANTINE_CACHE; a longer one
 * goes to a file instead.
 *
 * The operating system counts an environment entry as one string, and Linux
 * (MAX_ARG_STRLEN, 32 pages) refuses to start a process whose environment holds
 * a string of 128 KiB or more, with E2BIG at the `execve`. The assignment that
 * puts such a value there does not fail, so the run only breaks later, when the
 * next process starts: a Playwright worker that cannot spawn. This budget is
 * that limit without the name, the `=` and the terminating NUL, and a further
 * KiB of margin, because the whole environment also shares a limit of its own.
 */
const MAX_CACHE_BYTES = 128 * 1024 - CACHE_ENV.length - 2 - 1024;

/**
 * The last list this process read or fetched, for the calls it makes itself.
 * A list too long to go into the environment is also written to a file, so that
 * the processes this one spawns read the same list.
 */
let remembered: QuarantineItem[] | undefined;

/** The directory this process spilled to, made once and removed when it exits. */
let spillDir: string | undefined;

/**
 * Makes the directory this process spills to: mkdtemp gives this run its own,
 * readable only by its owner, so two runs on one machine cannot read each other's
 * list by guessing a name. The workers of this run are the file's only readers,
 * and they never outlive this process, which is when the exit hook removes it.
 *
 * @returns the path of the fresh directory
 */
function newSpillDir(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "flakeradar-quarantine-"));
  spillDir = dir;
  process.on("exit", () => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

/**
 * Exports a list that does not fit in one environment string, by writing it to a
 * private file under the system's temp dir and putting that file's path, which
 * is short, in FLAKERADAR_QUARANTINE_CACHE_FILE.
 *
 * @param cache the list as JSON
 */
function spill(cache: string): void {
  try {
    const file = path.join(spillDir ?? newSpillDir(), "cache.json");
    writeFileSync(file, cache, "utf8");
    process.env[CACHE_FILE_ENV] = file;
  } catch {
    // Nothing here may fail a run: without the file the list stays in this process
    // alone, and a later process asks for itself.
  }
}

/**
 * Writes the contract's one fail-open line and returns nothing to skip.
 *
 * A fetch error message can carry newlines, and the contract asks for one
 * line, so the whitespace inside the reason is collapsed.
 */
function skipNothing(reason: string): undefined {
  console.warn(`flakeradar-quarantine: ${reason.split(/\s+/).join(" ")}; skipping nothing`);
  return undefined;
}

/**
 * Whether the body is the array of items the endpoint answers, where every
 * element is an object with a string classname and a non-empty string name.
 * One invalid element invalidates the whole body.
 */
function isQuarantineList(body: unknown): body is QuarantineItem[] {
  return (
    Array.isArray(body) &&
    body.every(
      (item: unknown) =>
        typeof item === "object" &&
        item !== null &&
        typeof (item as QuarantineItem).classname === "string" &&
        typeof (item as QuarantineItem).name === "string" &&
        (item as QuarantineItem).name !== "",
    )
  );
}

/**
 * The items of one cache channel.
 *
 * @param cache the JSON a channel holds
 * @returns undefined when the text is not JSON or does not hold a valid body
 */
function parseCache(cache: string): QuarantineItem[] | undefined {
  try {
    const parsed: unknown = JSON.parse(cache);
    return isQuarantineList(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The items this process was handed: those in FLAKERADAR_QUARANTINE_CACHE, or,
 * when that variable is unset, those in the file named by
 * FLAKERADAR_QUARANTINE_CACHE_FILE. Undefined when neither is set, cannot be read,
 * or does not hold a valid body — a cache that cannot be read counts as absent, so
 * the list is fetched. A cache that holds the empty list is valid, and is what a
 * failed fetch leaves behind so the workers stay quiet.
 */
function cachedItems(): QuarantineItem[] | undefined {
  const cache = setting(CACHE_ENV);
  if (cache !== "") {
    return parseCache(cache);
  }
  const file = setting(CACHE_FILE_ENV);
  if (file === "") {
    return undefined;
  }
  try {
    return parseCache(readFileSync(file, "utf8"));
  } catch {
    return undefined;
  }
}

/**
 * Remembers a list as the answer of this run: in this process, and in one of the
 * two channels the processes this one spawns read — FLAKERADAR_QUARANTINE_CACHE
 * when the list is short enough for the environment, the file named by
 * FLAKERADAR_QUARANTINE_CACHE_FILE when it is not — so that a worker reads the
 * list instead of fetching it and skips exactly what this process skips.
 *
 * Playwright loads the config in the main process and again in every worker, so a
 * worker with no list to read asks the server a second time: it then costs a
 * request, writes a warning when the answer is bad, and may apply a list that is
 * not the one the run planned with.
 *
 * @param items the list this run answers with
 */
function remember(items: QuarantineItem[]): void {
  remembered = items;
  const cache = JSON.stringify(items);
  if (Buffer.byteLength(cache, "utf8") <= MAX_CACHE_BYTES) {
    process.env[CACHE_ENV] = cache;
  } else {
    spill(cache);
  }
}

/**
 * The reason a child run failed, in the words the contract writes for it.
 *
 * The child writes `HTTP <status>` to stderr and exits 1 on a non-2xx answer,
 * so that text names the status; anything else it wrote names the failure.
 */
function failureReason(error: unknown): string {
  const run = error as { signal?: string | null; stderr?: string; message?: string };
  if (run.signal) {
    return `FlakeRadar did not answer within 10 seconds (the request was killed with ${run.signal})`;
  }
  const stderr = (run.stderr || "").trim();
  if (stderr.startsWith("HTTP ")) {
    return `FlakeRadar answered ${stderr.slice(5)}`;
  }
  return `FlakeRadar cannot be read: ${stderr || run.message || "the request could not be run"}`;
}

/**
 * Fetches the quarantine list and returns the pattern that skips those tests.
 *
 * The cache comes first: a worker inherits the environment of the process that
 * loaded the config, so it reads what that load wrote — in the variable, or in
 * the file the variable names — and never fetches, and a later call in the
 * process that fetched reads what that call remembered. Only when the list has
 * to be fetched does this look at the server settings, and an unset URL or repo
 * ends it there, with no request, no cache and no warning.
 *
 * @returns undefined when nothing is skipped: the list could not be read, or is
 *   empty
 */
export function quarantineGrepInvert(): RegExp | undefined {
  const cached = cachedItems() ?? remembered;
  if (cached) {
    return buildGrepInvert(cached);
  }
  let url = setting("FLAKERADAR_URL");
  if (url.endsWith("/")) {
    url = url.slice(0, -1);
  }
  const repo = setting("FLAKERADAR_REPO") || setting("GITHUB_REPOSITORY");
  // Off: with no server or no repo there is nothing to ask, so nothing is
  // fetched, nothing is cached, and nothing is skipped.
  if (!url || !repo) {
    return undefined;
  }
  const project = setting("FLAKERADAR_PROJECT");
  const token = setting("FLAKERADAR_TOKEN");

  // URLSearchParams is Node's standard encoder, so acme/app goes out as
  // acme%2Fapp; an empty project is left out and the server uses its default.
  const query = new URLSearchParams({ repo });
  if (project) {
    query.set("project", project);
  }
  // The URL and the token travel as environment variables, never inside the
  // script, so neither can be re-read as code by the child.
  const script = [
    "const url = process.env.FLAKERADAR_QUARANTINE_REQUEST_URL;",
    'const token = process.env.FLAKERADAR_TOKEN || "";',
    'fetch(url, { headers: token ? { "X-API-Key": token } : {}, signal: AbortSignal.timeout(10_000) })',
    "  .then((res) => {",
    '    if (!res.ok) { console.error("HTTP " + res.status); process.exit(1); }',
    "    return res.text();",
    "  })",
    "  .then((text) => process.stdout.write(text))",
    "  .catch((error) => { console.error(String(error.message)); process.exit(1); });",
  ].join("\n");

  let body = "";
  try {
    body = execFileSync(process.execPath, ["-e", script], {
      timeout: 10_000,
      encoding: "utf8",
      // The child's stderr belongs to the reason below, not to the run: the
      // contract allows one line on stderr, so it is captured, not inherited.
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        FLAKERADAR_QUARANTINE_REQUEST_URL: `${url}/api/quarantine?${query}`,
        FLAKERADAR_TOKEN: token,
      },
    });
  } catch (error: unknown) {
    remember([]);
    return skipNothing(failureReason(error));
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    remember([]);
    return skipNothing("FlakeRadar's answer is not a list of items with a name");
  }
  if (!isQuarantineList(parsed)) {
    remember([]);
    return skipNothing("FlakeRadar's answer is not a list of items with a name");
  }
  remember(parsed);
  return buildGrepInvert(parsed);
}
