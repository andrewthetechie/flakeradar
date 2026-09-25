import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { breakable } from "./breakable";

describe("breakable", () => {
  it("adds a break opportunity after each separator", () => {
    const { container } = render(<span>{breakable("tests.test_a::run/x-y")}</span>);
    expect(container.innerHTML).toBe(
      "<span>tests.<wbr>test_<wbr>a:<wbr>:<wbr>run/<wbr>x-<wbr>y</span>",
    );
  });

  it("leaves plain words alone", () => {
    const { container } = render(<span>{breakable("renders")}</span>);
    expect(container.innerHTML).toBe("<span>renders</span>");
  });
});
