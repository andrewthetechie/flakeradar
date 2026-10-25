import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { TestCase } from "../api";
import { QuarantineDialog } from "./QuarantineDialog";

const test: TestCase = {
  id: 7,
  repo: "andrewthetechie/writers-app",
  project: "frontend",
  fingerprint: "f",
  suite: "unit",
  classname: "src/app.test.ts",
  name: "renders",
  file: "src/app.test.ts",
  line: 12,
  flakiness_score: 0.6,
  tier: "flaky",
  confirmed_flake_count: 1,
  failure_category: "assertion",
  clean_streak: 3,
  trend: "steady",
  last_status: "passed",
  last_seen_at: "2026-09-25T00:00:00Z",
  quarantined: false,
  quarantined_at: null,
  quarantine_reason: null,
  quarantine_expires_at: null,
  github_issue_number: null,
  github_issue_url: null,
};

function renderDialog(onClose = vi.fn(), onConfirm = vi.fn()) {
  render(<QuarantineDialog test={test} onClose={onClose} onConfirm={onConfirm} />);
  return { onClose, onConfirm };
}

describe("QuarantineDialog", () => {
  it("names the Test it is quarantining", () => {
    renderDialog();
    expect(screen.getByRole("dialog", { name: /Quarantine renders/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Quarantine renders/ })).toBeInTheDocument();
  });

  it("offers an optional reason capped at 500 characters", () => {
    renderDialog();
    const reason = screen.getByRole("textbox", { name: /Reason/ });
    expect(reason).toHaveAttribute("maxlength", "500");
  });

  it("offers Never / 7 / 14 / 30 days with Never checked by default", () => {
    renderDialog();
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("name"))).toEqual([
      "quarantine-expiry",
      "quarantine-expiry",
      "quarantine-expiry",
      "quarantine-expiry",
    ]);
    expect(radios.map((r) => r.closest("label")?.textContent)).toEqual([
      "Never",
      "7 days",
      "14 days",
      "30 days",
    ]);
    expect(screen.getByRole("radio", { name: "Never" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "14 days" })).not.toBeChecked();
  });

  it("confirms the defaults as no reason and no expiry", async () => {
    const { onConfirm, onClose } = renderDialog();
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledWith({ reason: null, expiresInDays: null });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("confirms the typed reason and the picked expiry", async () => {
    const { onConfirm } = renderDialog();
    await userEvent.click(screen.getByRole("radio", { name: "14 days" }));
    await userEvent.type(screen.getByRole("textbox", { name: /Reason/ }), "flaky");
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledWith({ reason: "flaky", expiresInDays: 14 });
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
