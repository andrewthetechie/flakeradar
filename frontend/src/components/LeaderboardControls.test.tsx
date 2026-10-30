import { fireEvent, render, screen, within } from "@testing-library/react";
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
const pipelineSelect = () => screen.queryByRole("combobox", { name: /pipeline/i });

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

  it("the new-flaky checkbox reports the window, or null when unticked", () => {
    const onNewDays = vi.fn();
    const { unmount } = render(
      <LeaderboardControls {...base} newDays={null} onNewDays={onNewDays} />,
    );
    // Rendered whenever the view wires it, i.e. in the Tests and Jobs views alike.
    fireEvent.click(screen.getByRole("checkbox", { name: /new in the last 7 days/i }));
    expect(onNewDays).toHaveBeenLastCalledWith(7);
    unmount();
    render(<LeaderboardControls {...base} newDays={7} onNewDays={onNewDays} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /new in the last 7 days/i }));
    expect(onNewDays).toHaveBeenLastCalledWith(null);
  });

  it("shows no new-flaky checkbox for a view without the handler", () => {
    render(<LeaderboardControls {...base} />);
    expect(
      screen.queryByRole("checkbox", { name: /new in the last 7 days/i }),
    ).not.toBeInTheDocument();
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

describe("LeaderboardControls pipeline filter", () => {
  it("lists All and one option per pipeline of the summary", () => {
    render(
      <LeaderboardControls
        {...base}
        pipelines={[".github/workflows/ci.yml", ".github/workflows/nightly.yml"]}
        pipeline={null}
        onPipeline={() => {}}
      />,
    );
    const select = screen.getByRole("combobox", { name: /pipeline/i });
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["All", ".github/workflows/ci.yml", ".github/workflows/nightly.yml"]);
    expect(select).toHaveValue("");
  });

  it("the pipeline select reports the name, and All reports null", () => {
    const onPipeline = vi.fn();
    render(
      <LeaderboardControls
        {...base}
        pipelines={["ci.yml"]}
        pipeline="ci.yml"
        onPipeline={onPipeline}
      />,
    );
    const select = screen.getByRole("combobox", { name: /pipeline/i });
    expect(select).toHaveValue("ci.yml");
    fireEvent.change(select, { target: { value: "" } });
    expect(onPipeline).toHaveBeenCalledWith(null);
  });

  it("shows no pipeline select for a view without the handler (tests)", () => {
    render(<LeaderboardControls {...base} issue={null} onIssue={() => {}} />);
    expect(pipelineSelect()).not.toBeInTheDocument();
  });
});

describe("LeaderboardControls sort and filter handlers", () => {
  it("the sort select reports the chosen key", () => {
    const onSort = vi.fn();
    render(<LeaderboardControls {...base} onSort={onSort} />);
    fireEvent.change(screen.getByRole("combobox", { name: /sort by/i }), {
      target: { value: "last_seen" },
    });
    expect(onSort).toHaveBeenCalledWith("last_seen");
  });

  it("the cause select reports the category, and Any reports null", () => {
    const onCause = vi.fn();
    render(<LeaderboardControls {...base} cause={null} causeCounts={null} onCause={onCause} />);
    const cause = screen.getByRole("combobox", { name: /likely cause/i });
    fireEvent.change(cause, { target: { value: "timing" } });
    expect(onCause).toHaveBeenLastCalledWith("timing");
    fireEvent.change(cause, { target: { value: "" } });
    expect(onCause).toHaveBeenLastCalledWith(null);
  });

  it("the pipeline select lists only All when the summary has no pipelines", () => {
    render(<LeaderboardControls {...base} onPipeline={() => {}} />);
    const select = screen.getByRole("combobox", { name: /pipeline/i });
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["All"]);
  });

  it("the stable checkbox reports the new value", () => {
    const onShowStable = vi.fn();
    render(<LeaderboardControls {...base} onShowStable={onShowStable} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /show stable/i }));
    expect(onShowStable).toHaveBeenCalledWith(true);
  });

  it("the quarantined box is unchecked when the view omits it", () => {
    render(<LeaderboardControls {...base} onQuarantined={() => {}} />);
    expect(screen.getByRole("checkbox", { name: /quarantined only/i })).not.toBeChecked();
  });
});
