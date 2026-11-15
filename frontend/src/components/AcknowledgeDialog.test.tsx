import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AcknowledgeDialog } from "./AcknowledgeDialog";

const name = "renders";
const scope = "andrewthetechie/writers-app / frontend";

function renderDialog(onClose = vi.fn(), onConfirm = vi.fn()) {
  render(<AcknowledgeDialog name={name} scope={scope} onClose={onClose} onConfirm={onConfirm} />);
  return { onClose, onConfirm };
}

describe("AcknowledgeDialog", () => {
  it("names the Test it is acknowledging", () => {
    renderDialog();
    expect(screen.getByRole("dialog", { name: /Acknowledge renders/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Acknowledge renders/ })).toBeInTheDocument();
    // The scope line goes through `breakable`, so match the dialog's whole text.
    expect(screen.getByRole("dialog", { name: /Acknowledge renders/ }).textContent).toContain(
      scope,
    );
  });

  it("offers an optional reason capped at 500 characters", () => {
    renderDialog();
    const reason = screen.getByRole("textbox", { name: /Reason/ });
    expect(reason).toHaveAttribute("maxlength", "500");
  });

  it("offers 1 / 7 / 30 days with 7 days checked by default", () => {
    renderDialog();
    const radios = screen.getAllByRole("radio");
    expect(radios).toHaveLength(3);
    expect(radios.map((r) => r.getAttribute("name"))).toEqual([
      "acknowledge-days",
      "acknowledge-days",
      "acknowledge-days",
    ]);
    expect(radios.map((r) => r.closest("label")?.textContent)).toEqual([
      "1 day",
      "7 days",
      "30 days",
    ]);
    expect(screen.getByRole("radio", { name: "7 days" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "1 day" })).not.toBeChecked();
  });

  it("confirms the defaults as no reason and 7 days", () => {
    const { onConfirm, onClose } = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm).toHaveBeenCalledWith({ reason: null, days: 7 });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("confirms the typed reason and the picked day count", async () => {
    const { onConfirm } = renderDialog();
    await userEvent.type(screen.getByRole("textbox", { name: /Reason/ }), "known flake");
    await userEvent.click(screen.getByRole("radio", { name: "30 days" }));
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm).toHaveBeenCalledWith({ reason: "known flake", days: 30 });
  });

  it("reports an all-space reason as null", () => {
    const { onConfirm } = renderDialog();
    fireEvent.change(screen.getByRole("textbox", { name: /Reason/ }), { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledWith({ reason: null, days: 7 });
  });

  it("closes on Cancel without confirming", async () => {
    const { onClose, onConfirm } = renderDialog();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("closes on Escape without confirming", () => {
    const { onClose, onConfirm } = renderDialog();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
