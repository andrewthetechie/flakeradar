import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TestCase } from "../api";
import { Leaderboard, scopeLabel } from "./Leaderboard";

const test: TestCase = {
  id: 1,
  repo: "andrewthetechie/writers-app",
  project: "backend",
  fingerprint: "f",
  suite: "unit",
  classname: "tests.test_views",
  name: "test_login",
  file: null,
  line: null,
  flakiness_score: 0.7,
  tier: "flaky",
  confirmed_flake_count: 2,
  failure_category: "timing",
  clean_streak: 0,
  trend: "worsening",
  last_status: "failed",
  last_seen_at: "2026-09-25T00:00:00Z",
  quarantined: false,
  quarantined_at: null,
  quarantine_reason: null,
  quarantine_expires_at: null,
  github_issue_number: null,
  github_issue_url: null,
};

describe("Leaderboard", () => {
  it("labels rows with repo · project when viewing all repos", () => {
    expect(scopeLabel(test, { repo: null, project: null })).toBe(
      "andrewthetechie/writers-app · backend",
    );
    expect(scopeLabel(test, { repo: test.repo, project: null })).toBe("backend");
    expect(scopeLabel(test, { repo: test.repo, project: "backend" })).toBeNull();
  });

  it("renders the label, tier and proof", () => {
    render(
      <Leaderboard
        tests={[test]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText("andrewthetechie/writers-app · backend")).toBeInTheDocument();
    expect(screen.getByText("flaky")).toBeInTheDocument();
    expect(screen.getByText("likely: timing")).toBeInTheDocument();
    expect(screen.getByLabelText(/^Worsening/)).toBeInTheDocument();
    expect(screen.getByText(/2× proven/)).toBeInTheDocument();
  });

  it("shows no trend mark and no likely-cause badge when there is none", () => {
    render(
      <Leaderboard
        tests={[{ ...test, trend: null, failure_category: null }]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.queryByText(/likely:/)).toBeNull();
    expect(screen.queryByLabelText(/Worsening|Improving|Steady/)).toBeNull();
  });

  it("explains the empty state when stable tests are hidden", () => {
    render(
      <Leaderboard
        tests={[]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText(/No flaky or suspect tests in this view/)).toBeInTheDocument();
  });

  it("says no tests match when a search returns nothing", () => {
    render(
      <Leaderboard
        tests={[]}
        scope={{ repo: null, project: null }}
        showStable={false}
        search="nope"
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText("No tests match.")).toBeInTheDocument();
    expect(screen.queryByText(/Tick “Show stable tests”/)).toBeNull();
  });

  it("tells you to POST a report when there is no data at all", () => {
    render(
      <Leaderboard
        tests={[]}
        scope={{ repo: null, project: null }}
        showStable
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    // The sentence is split around the <code> endpoint, so match a fragment.
    expect(screen.getByText(/No test data yet/)).toBeInTheDocument();
  });

  it("marks the selected row and no other", () => {
    const { unmount } = render(
      <Leaderboard
        tests={[test]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={test.id}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    // The selected-row class is the only signal that a row is selected.
    expect(screen.getByText(test.name).closest("tr")).toHaveClass("bg-surface-2");
    unmount();
    render(
      <Leaderboard
        tests={[test]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText(test.name).closest("tr")).not.toHaveClass("bg-surface-2");
  });

  it("selects a test when its row is clicked", () => {
    const onSelect = vi.fn();
    render(
      <Leaderboard
        tests={[test]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={onSelect}
        onToggleQuarantine={() => {}}
      />,
    );
    fireEvent.click(screen.getByText(test.name));
    expect(onSelect).toHaveBeenCalledWith(test.id);
  });

  it("shows the suite when the test has no classname", () => {
    render(
      <Leaderboard
        tests={[{ ...test, classname: "" }]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText("unit")).toBeInTheDocument();
  });

  it("renders a test that has never flipped", () => {
    render(
      <Leaderboard
        tests={[{ ...test, flakiness_score: 0 }]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={() => {}}
        onToggleQuarantine={() => {}}
      />,
    );
    expect(screen.getByText("0.00")).toBeInTheDocument();
  });

  it("does not select the row when its issue link is clicked", () => {
    const onSelect = vi.fn();
    render(
      <Leaderboard
        tests={[
          {
            ...test,
            github_issue_number: 9,
            github_issue_url: "https://github.com/acme/app/issues/9",
          },
        ]}
        scope={{ repo: null, project: null }}
        showStable={false}
        selectedId={null}
        onSelect={onSelect}
        onToggleQuarantine={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("link", { name: "#9" }));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
