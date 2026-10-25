import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { LeaderboardControls } from "./LeaderboardControls";

const base = {
  sort: "score" as const,
  showStable: false,
  search: "",
  onSort: () => {},
  onShowStable: () => {},
  onSearch: () => {},
};

const quarantinedBox = () => screen.queryByRole("checkbox", { name: /quarantined only/i });
const issueSelect = () => screen.queryByRole("combobox", { name: /issue/i });

describe("LeaderboardControls quarantine and issue filters", () => {
  it("shows both controls when the view provides their handlers", () => {
    render(
      <LeaderboardControls
        {...base}
        quarantined={false}
        issue={null}
        onQuarantined={() => {}}
        onIssue={() => {}}
      />,
    );
    expect(quarantinedBox()).toBeInTheDocument();
    expect(issueSelect()).toBeInTheDocument();
  });

  it("shows neither control for a view without the handlers (jobs)", () => {
    render(<LeaderboardControls {...base} />);
    expect(quarantinedBox()).not.toBeInTheDocument();
    expect(issueSelect()).not.toBeInTheDocument();
  });

  it("checking the box reports quarantined only", () => {
    const onQuarantined = vi.fn();
    render(<LeaderboardControls {...base} quarantined={false} onQuarantined={onQuarantined} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /quarantined only/i }));
    expect(onQuarantined).toHaveBeenCalledWith(true);
  });

  it("the issue select reports linked, none and any", () => {
    const onIssue = vi.fn();
    render(<LeaderboardControls {...base} issue={null} onIssue={onIssue} />);
    const select = screen.getByRole("combobox", { name: /issue/i });
    fireEvent.change(select, { target: { value: "linked" } });
    expect(onIssue).toHaveBeenLastCalledWith("linked");
    fireEvent.change(select, { target: { value: "none" } });
    expect(onIssue).toHaveBeenLastCalledWith("none");
    fireEvent.change(select, { target: { value: "" } });
    expect(onIssue).toHaveBeenLastCalledWith(null);
  });
});
