import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      // The classic, well-established hooks rules. We deliberately do NOT use
      // the plugin's `recommended`/`recommended-latest` bundles: those enable
      // the experimental React-Compiler-era rules (e.g. `set-state-in-effect`,
      // `purity`, `immutability`) which false-positive on the idiomatic
      // data-fetch-in-effect pattern used throughout this React 18 codebase.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // HMR hint only; does not affect correctness.
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],
    },
  },
  {
    // Vitest globals (describe/it/expect) used in test files.
    files: ["**/*.{test,spec}.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser, ...globals.vitest },
    },
  },
);
