// Vitest setup: jest-dom matchers (toBeInTheDocument, toBeDisabled, ...) and
// DOM cleanup between tests (not automatic without Vitest globals).
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => cleanup());
