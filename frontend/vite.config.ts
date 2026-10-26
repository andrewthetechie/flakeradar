/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": "http://localhost:8000",
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    // Coverage is measured on CI only. Local runs stay fast; opt in with
    // `npx vitest run --coverage.enabled --coverage.reporter=text`. Thresholds
    // are the measured baseline rounded down; raise them as coverage grows,
    // never down. CI also fails a PR whose figures fall below main's (ci.yml).
    coverage: {
      provider: "v8",
      enabled: Boolean(process.env.CI),
      include: ["src/**"],
      exclude: [
        "src/test/**",
        "src/**/*.test.{ts,tsx}",
        "src/**/*.css",
        // Boot glue: renders <App /> into #root.
        "src/main.tsx",
      ],
      reporter: ["text", "text-summary", "json-summary"],
      reportsDirectory: "test-results/coverage",
      // Measured at 90.80 / 83.42 / 85.20 / 92.94 when coverage was turned on.
      thresholds: {
        statements: 90,
        branches: 83,
        functions: 85,
        lines: 92,
      },
    },
  },
});
