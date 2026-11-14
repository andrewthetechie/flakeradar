import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Overlay } from "./Overlay";

const VARIANTS = ["drawer", "modal"] as const;

/** The class strings the drawers and the quarantine dialog spelled by hand. Pinned
 *  here so the shell cannot quietly restyle or re-tier an overlay it now owns. */
const CLASSES: Record<(typeof VARIANTS)[number], { scrim: string; panel: string }> = {
  drawer: {
    scrim: "fixed inset-0 z-20 bg-scrim",
    panel:
      "fixed inset-y-0 right-0 z-21 w-full max-w-190 animate-slide-in overflow-y-auto border-l border-line bg-surface px-5 pt-4 pb-8 shadow-2xl",
  },
  modal: {
    scrim: "fixed inset-0 z-30 bg-scrim",
    panel:
      "fixed inset-x-4 top-1/2 z-31 mx-auto w-full max-w-md -translate-y-1/2 rounded-lg border border-line bg-surface p-5 shadow-2xl",
  },
};

/** One overlay with the content a caller would pass. The scrim is the shell's first node. */
function overlay(variant: (typeof VARIANTS)[number], onClose = vi.fn()) {
  const { container } = render(
    <Overlay variant={variant} label="Test detail" onClose={onClose} breadcrumb="repo / project">
      <p>Body</p>
    </Overlay>,
  );
  return { onClose, scrim: container.firstElementChild as HTMLElement };
}

describe("Overlay", () => {
  it.each(VARIANTS)("closes on Escape (%s)", (variant) => {
    const { onClose } = overlay(variant);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it.each(VARIANTS)("closes on a scrim click (%s)", (variant) => {
    const { onClose, scrim } = overlay(variant);
    fireEvent.click(scrim);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it.each(VARIANTS)("closes on nothing else (%s)", (variant) => {
    const { onClose } = overlay(variant);
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it.each(VARIANTS)("exposes its label as the dialog name (%s)", (variant) => {
    overlay(variant);
    expect(screen.getByRole("dialog", { name: "Test detail" })).toBeInTheDocument();
    expect(screen.getByText("Body")).toBeInTheDocument();
  });

  it.each(VARIANTS)("keeps the scrim and panel classes it replaces (%s)", (variant) => {
    const { scrim } = overlay(variant);
    expect(scrim.className).toBe(CLASSES[variant].scrim);
    expect(screen.getByRole("dialog").className).toBe(CLASSES[variant].panel);
  });

  it("owns the × close affordance of a drawer", () => {
    const { onClose } = overlay("drawer");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.getByText("repo / project")).toBeInTheDocument();
  });

  it("leaves the centred dialog without a ×, whose Cancel is its content", () => {
    overlay("modal");
    expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
  });

  it("closes every mounted overlay on one Escape, as stacked overlays do today", () => {
    // App.tsx renders TestDrawer and QuarantineDialog as siblings, so the dialog
    // stacked on an open drawer shares one keydown: both close.
    const drawer = vi.fn();
    const modal = vi.fn();
    render(
      <Overlay variant="drawer" label="Test detail" onClose={drawer}>
        <p>Body</p>
      </Overlay>,
    );
    render(
      <Overlay variant="modal" label="Quarantine renders" onClose={modal}>
        <p>Body</p>
      </Overlay>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(drawer).toHaveBeenCalledTimes(1);
    expect(modal).toHaveBeenCalledTimes(1);
  });
});
