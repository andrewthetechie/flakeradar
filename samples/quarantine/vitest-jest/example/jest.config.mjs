// Jest reads the quarantine list when it loads this config and compiles the
// pattern again with the i flag, so a quarantined test never starts. The pattern
// arrives as a RegExp, so its source is what the config key wants; when the
// pattern is undefined — FlakeRadar unset or unreadable — the key is undefined
// too, which to Jest means "no filter". See ../flakeradar-quarantine.mjs.
import { quarantineTestNamePattern } from "../flakeradar-quarantine.mjs";

export default async () => ({
  testMatch: ["**/cart.test.js"],
  testNamePattern: (await quarantineTestNamePattern())?.source,
});
