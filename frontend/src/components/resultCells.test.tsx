import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ReactElement } from "react";
import {
  FlakinessCell,
  IssuePill,
  ProvenCell,
  QuarantinedPill,
  StatusIssueCell,
} from "./resultCells";

const bar = (container: HTMLElement) => container.querySelector(".h-full");
/** Cells that render their own <td> need a table around them to stay valid DOM. */
const inRow = (cell: ReactElement) => (
  <table>
    <tbody>
      <tr>{cell}</tr>
    </tbody>
  </table>
);

describe("FlakinessCell", () => {
  it("keeps a tiny positive score visible with the 4% bar floor", () => {
    const { container } = render(<FlakinessCell score={0.01} tier="suspect" />);
    expect(bar(container)).toHaveStyle("width: 4%");
  });

  it("renders no bar for a score of zero", () => {
    const { container } = render(<FlakinessCell score={0} tier="stable" />);
    expect(bar(container)).toHaveStyle("width: 0%");
  });

  it("shows the formatted score and the tier label", () => {
    render(<FlakinessCell score={0.6} tier="flaky" />);
    expect(screen.getByText("0.60")).toBeInTheDocument();
    const tier = screen.getByText("flaky");
    expect(tier.className).toContain("text-signal");
  });

  it("shows the likely-cause badge only when a category is given", () => {
    const { rerender } = render(<FlakinessCell score={0.6} tier="flaky" />);
    expect(screen.queryByText(/likely:/)).toBeNull();
    rerender(<FlakinessCell score={0.6} tier="flaky" category="timing" />);
    expect(screen.getByText("likely: timing")).toBeInTheDocument();
  });

  it("shows the trend mark only when a trend is given", () => {
    const { rerender } = render(<FlakinessCell score={0.6} tier="flaky" />);
    expect(screen.queryByLabelText(/^Worsening/)).toBeNull();
    rerender(<FlakinessCell score={0.6} tier="flaky" trend="worsening" />);
    expect(screen.getByLabelText(/^Worsening/)).toBeInTheDocument();
  });
});

describe("ProvenCell", () => {
  it("shows the proven count and its title", () => {
    render(inRow(<ProvenCell count={2} />));
    const proven = screen.getByText(/2× proven/);
    expect(proven).toHaveTextContent("⚠ 2× proven");
    expect(proven).toHaveAttribute("title", "Failed and passed on the same commit");
  });

  it("shows a dash when nothing is proven", () => {
    render(inRow(<ProvenCell count={0} />));
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.queryByText(/proven/)).toBeNull();
  });
});

describe("QuarantinedPill", () => {
  it("labels the quarantined pill", () => {
    render(<QuarantinedPill />);
    expect(screen.getByText("⏻ quarantined")).toHaveAttribute(
      "title",
      "Quarantined — runner may skip",
    );
  });
});

describe("IssuePill", () => {
  it("links the issue with the shared title", () => {
    render(<IssuePill number={42} url="https://github.com/acme/app/issues/42" />);
    const link = screen.getByRole("link", { name: "#42" });
    expect(link).toHaveAttribute("href", "https://github.com/acme/app/issues/42");
    expect(link).toHaveAttribute("title", "GitHub issue filed by FlakeRadar");
    expect(link.className).toBe("rounded-full border border-line px-2 text-link hover:underline");
  });

  it("keeps the host's extra pill classes", () => {
    render(<IssuePill number={42} url={null} className="text-xs" />);
    expect(screen.getByText("#42").className).toBe(
      "rounded-full border border-line px-2 text-xs text-link hover:underline",
    );
  });

  it("renders nothing when no issue is filed", () => {
    const { container } = render(<IssuePill number={null} url={null} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("StatusIssueCell", () => {
  it("renders the status and the linked pill", () => {
    render(
      inRow(
        <StatusIssueCell
          status="failed"
          issueNumber={7}
          issueUrl="https://github.com/acme/app/issues/7"
        />,
      ),
    );
    expect(screen.getByText("failed")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "#7" })).toHaveAttribute(
      "title",
      "GitHub issue filed by FlakeRadar",
    );
  });
});
