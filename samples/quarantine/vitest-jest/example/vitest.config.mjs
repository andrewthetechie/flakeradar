// Vitest reads the quarantine list when it builds this config, so a quarantined
// test never starts. With FLAKERADAR_URL unset the pattern is undefined, which
// to Vitest means "no filter": the Off case and the fail-open case both run all
// four tests. See ../flakeradar-quarantine.mjs.
import { defineConfig } from "vitest/config";

import { quarantineTestNamePattern } from "../flakeradar-quarantine.mjs";

export default defineConfig(async () => ({
  test: {
    globals: true,
    include: ["cart.test.js"],
    testNamePattern: await quarantineTestNamePattern(),
  },
}));
