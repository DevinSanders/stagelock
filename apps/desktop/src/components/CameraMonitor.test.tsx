import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CameraMonitor } from "./CameraMonitor";
import type { CameraConfig, CameraTelemetry } from "../types";

const BASE_CAM: CameraConfig = {
  id: "cam-1",
  name: "Stage Wide",
  enabled: true,
  source: { kind: "test_pattern", pattern: "smpte", width: 1280, height: 720, fps: 30 },
  ptz: { protocol: "mock" },
  ndi_output: { enabled: true, stream_name: "StageLock - Stage", include_color_adjustments: true },
  framing: {
    auto_track_enabled: true,
    preset: "waist_up",
    rule_of_thirds: "left_third",
    auto_focus_enabled: true,
    focus_strategy: "one_push_af",
    deadzone_x: 0.08,
    deadzone_y: 0.06,
    deadzone_zoom: 0.12,
    max_speed: 0.65,
    tally_damping_enabled: true,
  },
  color: {
    white_balance_temp: 0,
    white_balance_tint: 0,
    exposure: 0,
    contrast: 1,
    saturation: 1,
    sharpness: 0,
    brightness: 0,
  },
  vmix: { input_ref: "1" },
  presets: [],
};

const TELEMETRY: CameraTelemetry = {
  camera_id: "cam-1",
  timestamp_ms: 1000,
  stream_status: { state: "running", width: 1280, height: 720, fps: 30, hw_accelerated: true, decoder_name: "NVDEC" },
  ndi_output_active: true,
  tally: "program",
  tracking_state: "locked",
  auto_track_enabled: true,
  locked_track_id: 1,
  framing_preset: "waist_up",
  rule_of_thirds: "left_third",
  target_anchor: [0.333, 0.29],
  deadzone: { x: 0.25, y: 0.23, width: 0.16, height: 0.12 },
  focus_state: "sharp",
  focus_sharpness: 0.91,
  ptz_velocity: { pan: 0, tilt: 0, zoom: 0 },
  subjects: [
    {
      track_id: 1,
      bbox: { x: 0.25, y: 0.2, width: 0.2, height: 0.5 },
      confidence: 0.95,
      is_locked: true,
      identity_similarity: 0.94,
      keypoints: [{ x: 0.35, y: 0.28, score: 0.98 }],
      face_sharpness: 0.91,
    },
    {
      track_id: 2,
      bbox: { x: 0.65, y: 0.2, width: 0.18, height: 0.45 },
      confidence: 0.88,
      is_locked: false,
      keypoints: [{ x: 0.74, y: 0.27, score: 0.91 }],
    },
  ],
  preview_fps: 30,
  inference_fps: 30,
};

describe("CameraMonitor (telemetry HUD, tally borders, overlays & tap-to-track)", () => {
  it("renders tally program border, locked subject label, skeleton keypoints, NDI OUT, and handles tap-to-track", () => {
    vi.stubGlobal("VideoDecoder", undefined); // deterministic MJPEG fallback in unit test
    const onSelect = vi.fn();

    const { container } = render(
      <CameraMonitor
        camera={BASE_CAM}
        telemetry={TELEMETRY}
        showGrid={true}
        showSkeleton={true}
        onSelectPoint={onSelect}
      />
    );

    expect(container.querySelector(".tally-program")).toBeInTheDocument();
    expect(screen.getByText("Stage Wide")).toBeInTheDocument();
    expect(screen.getByText(/1280×720 @ 30fps · NVDEC/)).toBeInTheDocument();
    expect(screen.getByText("NDI OUT")).toBeInTheDocument();
    expect(screen.getByText("LOCKED")).toBeInTheDocument();
    expect(screen.getByText("MJPEG")).toBeInTheDocument();
    expect(screen.getByText("LOCK #1 · 94%")).toBeInTheDocument();
    expect(screen.getByText("Subject #2")).toBeInTheDocument();
    expect(screen.getByText("AF: 91%")).toBeInTheDocument();

    const videoWrap = container.querySelector(".monitor-video-wrap") as HTMLDivElement;
    videoWrap.getBoundingClientRect = () =>
      ({ left: 100, top: 50, width: 400, height: 200, right: 500, bottom: 250, x: 100, y: 50, toJSON: () => {} }) as DOMRect;

    fireEvent.click(videoWrap, { clientX: 300, clientY: 150 });
    expect(onSelect).toHaveBeenCalledWith(0.5, 0.5);

    // Out-of-bounds click clamps cleanly to [0, 1]
    fireEvent.click(videoWrap, { clientX: 999, clientY: -20 });
    expect(onSelect).toHaveBeenCalledWith(1, 0);
  });

  it("renders preview tally and default connecting state when telemetry is absent", () => {
    vi.stubGlobal("VideoDecoder", undefined);
    const { container, rerender } = render(
      <CameraMonitor camera={BASE_CAM} showGrid={false} showSkeleton={false} onSelectPoint={vi.fn()} />
    );
    expect(screen.getByText("connecting")).toBeInTheDocument();
    expect(screen.getByText("IDLE")).toBeInTheDocument();

    rerender(
      <CameraMonitor
        camera={BASE_CAM}
        telemetry={{ ...TELEMETRY, tally: "preview", tracking_state: "searching" }}
        showGrid={false}
        showSkeleton={false}
        onSelectPoint={vi.fn()}
      />
    );
    expect(container.querySelector(".tally-preview")).toBeInTheDocument();
    expect(screen.getByText("SEARCHING")).toBeInTheDocument();
  });
});
