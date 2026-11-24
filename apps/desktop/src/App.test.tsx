import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { api } from "./api";
import { setViewportCompact } from "./test/setup";
import type { CameraConfig, CameraTelemetry, SystemCapabilities, VisionEngineStatus } from "./types";

class MockTelemetryWs {
  static instances: MockTelemetryWs[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  close = vi.fn();
  constructor(public url: string) {
    MockTelemetryWs.instances.push(this);
  }
}

const CAM1: CameraConfig = {
  id: "cam-1",
  name: "Cam 1 — Stage",
  enabled: true,
  source: { kind: "test_pattern", pattern: "smpte", width: 1280, height: 720, fps: 30 },
  ptz: { protocol: "mock" },
  ndi_output: { enabled: false, stream_name: "VC - Cam 1", include_color_adjustments: true },
  framing: {
    auto_track_enabled: false,
    preset: "waist_up",
    rule_of_thirds: "center",
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
  vmix: {},
  presets: [{ slot: 1, name: "Wide Stage" }],
};

const CAM2: CameraConfig = { ...CAM1, id: "cam-2", name: "Cam 2 — Pulpit" };

const CAPS: SystemCapabilities = {
  os: "windows",
  arch: "x86_64",
  gpu_names: ["NVIDIA GeForce RTX 5080"],
  decode_backend: "nvidia_nvcodec",
  encode_backend: "nvidia_nvcodec",
  available_decode_backends: ["nvidia_nvcodec", "software_cpu"],
  available_encode_backends: ["nvidia_nvcodec", "software_cpu"],
  inference_provider: "cuda",
  gstreamer_available: true,
  gstreamer_version: "1.28.9",
  ndi_runtime_available: true,
};

const VISION: VisionEngineStatus = {
  onnx_feature_compiled: true,
  active_execution_provider: "CUDA",
  models_dir: "models",
  detector_ready: true,
  pose_ready: false,
  reid_ready: false,
  models: [{ role: "Detector", filename: "rtmdet_s.onnx", installed: true, license: "Apache-2.0" }],
};

const TELEM1: CameraTelemetry = {
  camera_id: "cam-1",
  timestamp_ms: 1000,
  stream_status: { state: "running", width: 1280, height: 720, fps: 30, hw_accelerated: true, decoder_name: "NVDEC" },
  ndi_output_active: true,
  tally: "program",
  tracking_state: "locked",
  auto_track_enabled: true,
  locked_track_id: 1,
  framing_preset: "waist_up",
  rule_of_thirds: "center",
  target_anchor: [0.5, 0.29],
  deadzone: { x: 0.42, y: 0.23, width: 0.16, height: 0.12 },
  focus_state: "sharp",
  focus_sharpness: 0.88,
  ptz_velocity: { pan: 0.12, tilt: -0.05, zoom: 0 },
  subjects: [
    {
      track_id: 1,
      bbox: { x: 0.4, y: 0.2, width: 0.2, height: 0.5 },
      confidence: 0.96,
      is_locked: true,
      identity_similarity: 0.95,
      keypoints: [],
    },
  ],
  preview_fps: 30,
  inference_fps: 30,
};

describe("App end-to-end user workflows (Desktop Studio & Mobile PWA, happy & failure paths)", () => {
  beforeEach(() => {
    setViewportCompact(false);
    MockTelemetryWs.instances = [];
    vi.stubGlobal("WebSocket", MockTelemetryWs);
    vi.stubGlobal("VideoDecoder", undefined);
    vi.spyOn(api, "listCameras").mockResolvedValue([CAM1, CAM2]);
    vi.spyOn(api, "getCapabilities").mockResolvedValue(CAPS);
    vi.spyOn(api, "getVisionStatus").mockResolvedValue(VISION);
    vi.spyOn(api, "getRemoteInfo").mockResolvedValue({
      lan_access_enabled: true,
      http_port: 8765,
      osc_port: 9000,
      pwa_ready: true,
      lan_urls: ["http://10.7.21.2:8765"],
      local_url: "http://127.0.0.1:8765",
    });
    vi.spyOn(api, "discoverNdi").mockResolvedValue({ available: true, sources: [] });
    vi.spyOn(api, "getVmixState").mockResolvedValue({
      enabled: false,
      connected: false,
      host: "127.0.0.1",
      http_port: 8088,
      tcp_port: 8099,
      recording: false,
      streaming: false,
      tally_string: "",
      inputs: [],
    });
    vi.spyOn(api, "getStreamDeckStatus").mockResolvedValue({
      direct_hid_enabled: false,
      brightness: 80,
      connected_devices: [],
      mode_note: "Companion mode",
      key_bindings: [],
      dial_bindings: [],
    });
    vi.spyOn(api, "getButtonsConfig").mockResolvedValue({
      enabled: false,
      base_url: "http://127.0.0.1:8000",
      api_key: "",
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("drives full desktop studio workflow: telemetry, AI tracking, calibration (success & error), PTZ, Color/NDI, modals, and camera delete", async () => {
    const updateFraming = vi.spyOn(api, "updateFraming").mockResolvedValue(CAM1);
    const selectSubject = vi.spyOn(api, "selectSubject").mockResolvedValue({ locked_track_id: 1 });
    const ptzFocus = vi.spyOn(api, "ptzFocus").mockResolvedValue(undefined);
    const ptzCal = vi.spyOn(api, "ptzCalibrate")
      .mockRejectedValueOnce(new Error("HTTP 400: No PTZ driver"))
      .mockResolvedValueOnce({
        camera_id: "cam-1",
        protocol: "mock",
        calibration: { pan_scale: 1, tilt_scale: 1, zoom_scale: 1, latency_ms: 155, min_effective_speed: 0.03 },
        note: "ok",
      });
    const ptzMove = vi.spyOn(api, "ptzMove").mockResolvedValue(undefined);
    const ptzStop = vi.spyOn(api, "ptzStop").mockResolvedValue(undefined);
    const ptzPreset = vi.spyOn(api, "ptzPreset").mockResolvedValue(undefined);
    const updateNdi = vi.spyOn(api, "updateNdiOutput").mockResolvedValue(CAM1);
    const updateColor = vi.spyOn(api, "updateColor").mockResolvedValue(CAM1);
    const cameraImaging = vi.spyOn(api, "cameraImaging").mockResolvedValue(undefined);
    const createCam = vi.spyOn(api, "createCamera").mockResolvedValue({ ...CAM1, id: "cam-3", name: "Camera 3" });
    const deleteCam = vi.spyOn(api, "deleteCamera").mockResolvedValue(undefined);

    render(<App />);
    const ws = MockTelemetryWs.instances[0];
    act(() => {
      ws.onopen?.();
      ws.onmessage?.({ data: "{bad_json" });
      ws.onmessage?.({ data: JSON.stringify({ type: "capabilities", ...CAPS }) });
      ws.onmessage?.({ data: JSON.stringify({ type: "telemetry_batch", cameras: [TELEM1] }) });
      ws.onmessage?.({ data: JSON.stringify({ type: "cameras_updated" }) });
    });

    await waitFor(() => expect(screen.getByText("Engine Online")).toBeInTheDocument());
    expect(screen.getByText(/NDI® 6 Ready/)).toBeInTheDocument();

    // Topbar toggles & modals
    fireEvent.click(screen.getByRole("button", { name: "Guides" }));
    expect(screen.getByRole("button", { name: "No Guides" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pose On" }));
    expect(screen.getByRole("button", { name: "Pose Off" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remote QR" }));
    await waitFor(() => expect(screen.getByText(/Remote Control PWA/i)).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Close" }));

    fireEvent.click(screen.getByRole("button", { name: "Setup" }));
    expect(screen.getByText(/System Readiness/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /\+ Open Add Camera Wizard/i }));
    fireEvent.click(screen.getByRole("button", { name: "Add Camera" }));
    await waitFor(() => expect(createCam).toHaveBeenCalledTimes(1));

    // Switch back to Cam 1 via multiview tile
    fireEvent.click(screen.getAllByText("Cam 1 — Stage")[0]);

    // AI Track controls: enable track, presets, thirds, subject lock/clear, AF controls, sliders, tally damping
    fireEvent.click(screen.getByRole("button", { name: /ENABLE AUTO TRACKING/i }));
    fireEvent.click(screen.getByRole("button", { name: "Close-Up" }));
    fireEvent.click(screen.getByRole("button", { name: "Left 1/3" }));
    fireEvent.click(screen.getByRole("button", { name: "Locked #1" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear Lock" }));
    fireEvent.click(screen.getByRole("button", { name: "Auto-AF On" }));
    fireEvent.click(screen.getByRole("button", { name: "One-Push AF" }));
    fireEvent.click(screen.getByRole("button", { name: "Tier 2: Hill-Climb AF" }));

    const focusNear = screen.getByRole("button", { name: "Focus Near" });
    const focusFar = screen.getByRole("button", { name: "Focus Far" });
    focusNear.setPointerCapture = vi.fn();
    focusFar.setPointerCapture = vi.fn();
    fireEvent.pointerDown(focusNear, { pointerId: 1 });
    fireEvent.pointerUp(focusNear);
    fireEvent.pointerDown(focusFar, { pointerId: 2 });
    fireEvent.pointerCancel(focusFar);

    const ranges = screen.getAllByRole("slider");
    fireEvent.change(ranges[0], { target: { value: "0.8" } });
    fireEvent.change(ranges[1], { target: { value: "0.1" } });
    fireEvent.click(screen.getByRole("checkbox"));

    expect(updateFraming).toHaveBeenCalled();
    expect(selectSubject).toHaveBeenCalledWith("cam-1", { track_id: 1 });
    expect(selectSubject).toHaveBeenCalledWith("cam-1", { clear: true });
    expect(ptzFocus).toHaveBeenCalledWith("cam-1", { action: "one_push" });

    // Calibration failure path -> shows alert; then success path -> shows calibrated report
    fireEvent.click(screen.getByRole("button", { name: /Run PTZ Speed & Latency Calibration/i }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("HTTP 400: No PTZ driver"));
    fireEvent.click(screen.getByRole("button", { name: /Run PTZ Speed & Latency Calibration/i }));
    await waitFor(() => expect(screen.getByText(/Calibrated \(mock\): latency 155 ms/i)).toBeInTheDocument());
    expect(ptzCal).toHaveBeenCalledTimes(2);

    // PTZ Tab
    fireEvent.click(screen.getByRole("button", { name: "PTZ" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop All" }));
    fireEvent.click(screen.getByRole("button", { name: "#1 Wide Stage" }));
    const zoomBtn = screen.getByRole("button", { name: /Zoom In/i });
    zoomBtn.setPointerCapture = vi.fn();
    fireEvent.pointerDown(zoomBtn, { pointerId: 4 });
    expect(ptzMove).toHaveBeenCalledWith("cam-1", 0, 0, 0.55);
    expect(ptzStop).toHaveBeenCalled();
    expect(ptzPreset).toHaveBeenCalledWith("cam-1", "recall", 1);

    // Color / NDI Tab (NDI out, Layer-1 imaging, Layer-2 grading, Reset, Remove Camera)
    fireEvent.click(screen.getByRole("button", { name: "Color / NDI" }));
    fireEvent.click(screen.getByRole("button", { name: "Start NDI Out" }));
    fireEvent.change(screen.getByDisplayValue("VC - Cam 1"), { target: { value: "VC - Main" } });
    fireEvent.click(screen.getByRole("checkbox"));
    expect(updateNdi).toHaveBeenCalled();

    for (const label of ["WB Auto", "Indoor", "Outdoor", "1-Push WB", "AE Auto", "BLC On", "BLC Off"]) {
      fireEvent.click(screen.getByRole("button", { name: label }));
    }
    const videoSliders = screen.getAllByRole("slider");
    fireEvent.mouseUp(videoSliders[0]);
    fireEvent.touchEnd(videoSliders[0]);
    fireEvent.change(videoSliders[1], { target: { value: "0.25" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(cameraImaging).toHaveBeenCalled();
    expect(updateColor).toHaveBeenCalled();

    // Integrations tab
    fireEvent.click(screen.getByRole("button", { name: "vMix / Deck" }));
    expect(screen.getByText(/vMix Live Switcher/i)).toBeInTheDocument();

    // Return to Color/NDI and delete camera
    fireEvent.click(screen.getByRole("button", { name: "Color / NDI" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Camera Source" }));
    await waitFor(() => expect(deleteCam).toHaveBeenCalledWith("cam-1"));

    // WebSocket disconnect & error triggers Reconnecting state cleanly
    act(() => {
      ws.onerror?.();
      ws.onclose?.();
    });
    expect(screen.getByText("Reconnecting...")).toBeInTheDocument();
  });

  it("renders responsive Mobile/Tablet PWA bottom navigation, 1-tap camera pills, and multiview tab", async () => {
    setViewportCompact(true);
    render(<App />);
    const ws = MockTelemetryWs.instances[0];
    act(() => {
      ws.onopen?.();
    });

    await waitFor(() => expect(screen.getByRole("button", { name: "Cams" })).toBeInTheDocument());
    expect(screen.getByText("Remote PWA")).toBeInTheDocument();

    // Tap camera pill switcher
    fireEvent.click(screen.getByRole("button", { name: /Cam 2 — Pulpit/i }));
    // Navigate mobile bottom nav tabs: Cams -> tap tile returns to Track; PTZ, Color, vMix
    fireEvent.click(screen.getByRole("button", { name: "Cams" }));
    expect(screen.getByText("All Cameras Live Multiview")).toBeInTheDocument();
    const tiles = screen.getAllByText("Cam 1 — Stage");
    fireEvent.click(tiles[tiles.length - 1]);
    expect(screen.getByText("Auto Tracking")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "PTZ" }));
    fireEvent.click(screen.getByRole("button", { name: "Color" }));
    fireEvent.click(screen.getByRole("button", { name: "vMix" }));
    fireEvent.click(screen.getByRole("button", { name: "Track" }));
  });
});
