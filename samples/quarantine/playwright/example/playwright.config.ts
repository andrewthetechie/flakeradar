// The sample config. grepInvert drops a quarantined test before it starts, so
// the suite that runs is the quarantine list subtracted from the suite that
// exists. See ../flakeradar-quarantine.ts for the helper and ../README.md for
// the contract.
//
// The call stays synchronous on purpose: the config loader validates the module's
// default export without awaiting it, so a Promise config is rejected and the
// helper fetches in a child process it can wait for. `undefined`, which is what
// an off run and a fail-open run return, is simply no filter.
//
// The names the pattern is built from are the ones the JUnit reporter writes with
// includeProjectInTestName at its default, which is what FlakeRadar stores, so
// this file keeps that option at its default too.

import { defineConfig } from "@playwright/test";

import { quarantineGrepInvert } from "../flakeradar-quarantine.ts";

export default defineConfig({
  testDir: ".",
  grepInvert: quarantineGrepInvert(),
  projects: [{ name: "unit" }],
});
