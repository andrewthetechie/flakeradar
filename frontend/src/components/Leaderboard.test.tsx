import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TestCase } from "../api";
import { Leaderboard, scopeLabel } from "./Leaderboard";

const test: TestCase = {
  id: 1, repo: "andrewthetechie/writers-app", project: "backend", fingerprint: "f",
  suite: "unit", classname: "tests.test_views", name: "test_login", file: null, line: null,
  flakiness_score: 0.7, tier: "flaky", confirmed_flake_count: 2, last_status: "failed",
  last_seen_at: "2026-09-25T00:00:00Z", quarantined: false, quarantined_at: null,
  github_issue_number: null,
};

describe("Leaderboard", () => {
  it("labels rows with repo · project when viewing all repos", () => {
    expect(scopeLabel(test, { repo: null, project: null }))
      .toBe("andrewthetechie/writers-app · backend");
    expect(scopeLabel(test, { repo: test.repo, project: null })).toBe("backend");
    expect(scopeLabel(test, { repo: test.repo, project: "backend" })).toBeNull();
  });

  it("renders the label, tier and proof", () => {
    render(
      <Leaderboard tests={[test]} scope={{ repo: null, project: null }} showStable={false}
        selectedId={null} onSelect={() => {}} onToggleQuarantine={() => {}} />,
    );
    expect(screen.getByText("andrewthetechie/writers-app · backend")).toBeInTheDocument();
    expect(screen.getByText("flaky")).toBeInTheDocument();
    expect(screen.getByText(/2× proven/)).toBeInTheDocument();
  });

  it("explains the empty state when stable tests are hidden", () => {
    render(
      <Leaderboard tests={[]} scope={{ repo: null, project: null }} showStable={false}
        selectedId={null} onSelect={() => {}} onToggleQuarantine={() => {}} />,
    );
    expect(screen.getByText(/No flaky or suspect tests in this view/)).toBeInTheDocument();
  });
});
