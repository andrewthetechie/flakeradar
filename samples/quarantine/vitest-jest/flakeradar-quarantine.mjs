// The Vitest and Jest quarantine helper, documented at ../README.md.
//
// Copy this file next to your vitest.config.mjs or jest.config.mjs and use it
// as shown above buildTestNamePattern. It needs Node 18 or later for the
// global fetch, and nothing else.
//
// The environment is FLAKERADAR_URL, FLAKERADAR_TOKEN, FLAKERADAR_REPO
// (falling back to GITHUB_REPOSITORY) and FLAKERADAR_PROJECT; an empty
// variable counts as unset. When the URL or the repo is unset nothing is
// fetched and nothing is skipped. On any fetch, status or body problem one
// warning line goes to stderr and nothing is skipped either: this helper never
// fails a run because of FlakeRadar.

/**
 * A quarantined test, as `GET /api/quarantine` answers it. Only `name` is used
 * here; the other keys are what the endpoint writes, listed in ../README.md.
 *
 * @typedef {object} QuarantineItem
 * @property {string} classname
 * @property {string} name
 */

// Builds the RegExp that skips every quarantined test, or undefined when there
// is nothing to skip. Each name is trimmed first: jest-junit's default
// classNameTemplate is "{classname} {title}", so a top-level Jest test reaches
// the API with a leading space, while Jest matches the pattern against the
// space-joined names without it. Trimming is what makes one helper serve both
// runners.
//
// Example input, this sample's vitest/quarantine.json:
//
//   [
//     {"suite": "cart.test.js", "classname": "cart.test.js",
//      "name": "cart > adds an item", "fingerprint": "0000000000000000000000000000000000000001",
//      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
//      "reason": "flaky in CI", "expires_at": "2026-12-01T12:00:00.000000Z"},
//     {"suite": "cart.test.js", "classname": "cart.test.js",
//      "name": "checkout total", "fingerprint": "0000000000000000000000000000000000000002",
//      "file": null, "line": null, "quarantined_at": "2026-11-01T12:00:00.000000Z",
//      "reason": null, "expires_at": null}
//   ]
//
// returns a RegExp whose source is exactly:
//
//   ^(?!(?:cart > adds an item|checkout total)$)
//
// Usage in vitest.config.mjs, where testNamePattern wants the pattern itself:
//
//   export default defineConfig(async () => ({ test: { testNamePattern: await quarantineTestNamePattern() } }));
//
// Usage in jest.config.mjs, where testNamePattern wants the pattern's source:
//
//   export default async () => ({ testNamePattern: (await quarantineTestNamePattern())?.source });
//
// Jest compiles that source with the i flag, so a test whose name differs from
// a quarantined one only in case is skipped too.
//
// The pattern sees names, not files, so a test of the same name in another
// file is skipped as well. Scope the quarantine by name, or leave it out.
//
// @param {readonly QuarantineItem[]} items the quarantine list, in list order
// @returns {RegExp | undefined} undefined when the list holds no usable name
export function buildTestNamePattern(items) {
  const names = [];
  for (const item of items) {
    const name = item.name.trim();
    if (name !== "" && !names.includes(name)) {
      names.push(name);
    }
  }
  if (names.length === 0) {
    return undefined;
  }
  const escaped = names.map((name) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  return new RegExp("^(?!(?:" + escaped.join("|") + ")$)");
}

/**
 * The value of an environment variable, or "" when it is unset or empty.
 *
 * @param {string} name
 * @returns {string}
 */
function setting(name) {
  return process.env[name] || "";
}

/**
 * Writes the contract's one fail-open line and returns nothing to skip.
 *
 * A fetch error message can carry newlines, and the contract asks for one
 * line, so the whitespace inside the reason is collapsed.
 *
 * @param {string} reason
 * @returns {undefined}
 */
function skipNothing(reason) {
  console.warn(`flakeradar-quarantine: ${reason.split(/\s+/).join(" ")}; skipping nothing`);
  return undefined;
}

/**
 * Whether the body is the array of items the endpoint answers, where every
 * element is an object with a string classname and a non-empty string name.
 * One invalid element invalidates the whole body.
 *
 * @param {unknown} body
 * @returns {body is QuarantineItem[]}
 */
function isQuarantineList(body) {
  return (
    Array.isArray(body) &&
    body.every(
      (item) =>
        typeof item === "object" &&
        item !== null &&
        typeof (/** @type {QuarantineItem} */ (item).classname) === "string" &&
        typeof item.name === "string" &&
        item.name !== "",
    )
  );
}

/**
 * Fetches the quarantine list and returns the pattern that skips those tests.
 *
 * @returns {Promise<RegExp | undefined>} undefined when nothing is skipped: the
 *   list could not be read, or holds no usable name
 */
export async function quarantineTestNamePattern() {
  let url = setting("FLAKERADAR_URL");
  if (url.endsWith("/")) {
    url = url.slice(0, -1);
  }
  const repo = setting("FLAKERADAR_REPO") || setting("GITHUB_REPOSITORY");
  // Off: with no server or no repo there is nothing to ask, so nothing is
  // fetched, nothing is written, and nothing is skipped.
  if (!url || !repo) {
    return undefined;
  }
  const project = setting("FLAKERADAR_PROJECT");
  const token = setting("FLAKERADAR_TOKEN");

  try {
    // URLSearchParams is Node's standard encoder, so acme/app goes out as
    // acme%2Fapp; an empty project is left out and the server uses its default.
    const query = new URLSearchParams({ repo });
    if (project) {
      query.set("project", project);
    }
    const response = await fetch(`${url}/api/quarantine?${query}`, {
      headers: token ? { "X-API-Key": token } : {},
      signal: AbortSignal.timeout(10_000),
    });
    if (!response.ok) {
      return skipNothing(`FlakeRadar answered ${response.status}`);
    }
    const body = await response.json();
    if (!isQuarantineList(body)) {
      return skipNothing("FlakeRadar's answer is not a list of items with a name");
    }
    return buildTestNamePattern(body);
  } catch (error) {
    // A connection error, a timeout, or a body that is not JSON. Nothing here
    // may reach the caller, whose config would then fail the run.
    const message = error instanceof Error ? error.message : String(error);
    return skipNothing(`FlakeRadar cannot be read: ${message}`);
  }
}
