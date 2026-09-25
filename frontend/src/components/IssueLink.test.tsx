import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { IssueLink } from "./IssueLink";

describe("IssueLink", () => {
  it("renders nothing when no issue is filed", () => {
    const { container } = render(
      <IssueLink number={null} url={null}>
        #0
      </IssueLink>,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("links to the filed issue", () => {
    render(
      <IssueLink number={42} url="https://github.com/acme/app/issues/42">
        #42
      </IssueLink>,
    );
    const link = screen.getByRole("link", { name: "#42" });
    expect(link).toHaveAttribute("href", "https://github.com/acme/app/issues/42");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noreferrer");
  });
});
