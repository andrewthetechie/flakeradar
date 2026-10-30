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

  it("renders an anchor without href when no url is known", () => {
    render(
      <IssueLink number={3} url={null}>
        #3
      </IssueLink>,
    );
    const anchor = screen.getByText("#3");
    expect(anchor.tagName).toBe("A");
    expect(anchor).not.toHaveAttribute("href");
  });
});
