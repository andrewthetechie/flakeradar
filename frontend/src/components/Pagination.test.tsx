import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Pagination } from "./Pagination";

describe("Pagination", () => {
  it("shows the position and moves forward", async () => {
    const onPage = vi.fn();
    render(<Pagination page={2} pageSize={50} total={123} onPage={onPage} />);
    expect(screen.getByText("Page 2 of 3 · 51–100 of 123")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Next ›" }));
    expect(onPage).toHaveBeenCalledWith(3);
  });

  it("disables prev on the first page and next on the last", () => {
    const { rerender } = render(<Pagination page={1} pageSize={50} total={60} onPage={() => {}} />);
    expect(screen.getByRole("button", { name: "‹ Prev" })).toBeDisabled();
    rerender(<Pagination page={2} pageSize={50} total={60} onPage={() => {}} />);
    expect(screen.getByRole("button", { name: "Next ›" })).toBeDisabled();
  });
});
