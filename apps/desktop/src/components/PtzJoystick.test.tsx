import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PtzJoystick } from "./PtzJoystick";

const PRESETS = [
  { slot: 1, name: "Wide Stage" },
  { slot: 2, name: "Pulpit" },
];

describe("PtzJoystick (user pointer drag, zoom hold, presets recall & store)", () => {
  it("clamps joystick drag to pad radius and stops cleanly on release", () => {
    const onMove = vi.fn();
    const onStop = vi.fn();
    const onPreset = vi.fn();

    const { container } = render(
      <PtzJoystick presets={PRESETS} onMove={onMove} onStop={onStop} onPreset={onPreset} />
    );

    const pad = container.querySelector(".joystick-pad") as HTMLDivElement;
    pad.setPointerCapture = vi.fn();
    pad.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 140, height: 140, right: 140, bottom: 140, x: 0, y: 0, toJSON: () => {} }) as DOMRect;

    // Pointer move before pointerDown is ignored
    fireEvent.pointerMove(pad, { clientX: 120, clientY: 70 });
    expect(onMove).not.toHaveBeenCalled();

    // Drag far right-up beyond maxR (50px) -> clamps to pan=1, tilt=0
    fireEvent.pointerDown(pad, { pointerId: 1, clientX: 250, clientY: 70 });
    expect(onMove).toHaveBeenLastCalledWith(1, 0, 0);

    // Move inside radius
    fireEvent.pointerMove(pad, { pointerId: 1, clientX: 70, clientY: 45 });
    expect(onMove).toHaveBeenLastCalledWith(0, 0.5, 0);

    // Context menu prevented
    const ctx = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    pad.dispatchEvent(ctx);
    expect(ctx.defaultPrevented).toBe(true);

    // Release resets and calls onStop
    fireEvent.pointerUp(pad);
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("handles Zoom In / Zoom Out press-and-hold, Stop All, and Preset Recall/Store toggle", () => {
    const onMove = vi.fn();
    const onStop = vi.fn();
    const onPreset = vi.fn();

    render(<PtzJoystick presets={PRESETS} onMove={onMove} onStop={onStop} onPreset={onPreset} />);

    const zoomIn = screen.getByRole("button", { name: /Zoom In/i });
    const zoomOut = screen.getByRole("button", { name: /Zoom Out/i });
    zoomIn.setPointerCapture = vi.fn();
    zoomOut.setPointerCapture = vi.fn();

    fireEvent.pointerDown(zoomIn, { pointerId: 2 });
    expect(onMove).toHaveBeenCalledWith(0, 0, 0.55);
    fireEvent.pointerUp(zoomIn);
    expect(onStop).toHaveBeenCalledTimes(1);

    fireEvent.pointerDown(zoomOut, { pointerId: 3 });
    expect(onMove).toHaveBeenCalledWith(0, 0, -0.55);
    fireEvent.pointerCancel(zoomOut);
    expect(onStop).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole("button", { name: "Stop All" }));
    expect(onStop).toHaveBeenCalledTimes(3);

    // Recall preset #1
    fireEvent.click(screen.getByRole("button", { name: "#1 Wide Stage" }));
    expect(onPreset).toHaveBeenCalledWith("recall", 1);

    // Switch to Store mode, store preset #2, verify auto-reset to Recall
    fireEvent.click(screen.getByRole("button", { name: "Mode: Recall" }));
    expect(screen.getByRole("button", { name: "Mode: STORE" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "#2 Pulpit" }));
    expect(onPreset).toHaveBeenCalledWith("store", 2);
    expect(screen.getByRole("button", { name: "Mode: Recall" })).toBeInTheDocument();
  });
});
