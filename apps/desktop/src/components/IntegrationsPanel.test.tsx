import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { IntegrationsPanel } from "./IntegrationsPanel";
import type { CameraConfig, StreamDeckStatus, VmixRuntimeState } from "../types";

const CAM: CameraConfig = {
  id: "cam-1",
  name: "Stage Wide",
  enabled: true,
  source: { kind: "test_pattern", pattern: "smpte", width: 1280, height: 720, fps: 30 },
  ptz: { protocol: "mock" },
  ndi_output: { enabled: false, stream_name: "StageLock - Stage", include_color_adjustments: true },
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
  vmix: { input_ref: "1" },
  presets: [],
};

const VMIX_CONNECTED: VmixRuntimeState = {
  enabled: true,
  connected: true,
  host: "10.0.0.50",
  http_port: 8088,
  tcp_port: 8099,
  version: "28.0.0.42",
  active_input: 1,
  preview_input: 2,
  recording: true,
  streaming: true,
  tally_string: "120",
  inputs: [
    { key: "k1", number: 1, kind: "NDI", title: "Stage Wide", short_title: "Wide", state: "Running", tally: "program" },
    { key: "k2", number: 2, kind: "NDI", title: "Pulpit", short_title: "Pulpit", state: "Running", tally: "preview" },
    { key: "k3", number: 3, kind: "NDI", title: "Rear", short_title: "Rear", state: "Running", tally: "off" },
  ],
};

const SD_STATUS: StreamDeckStatus = {
  direct_hid_enabled: false,
  brightness: 80,
  connected_devices: [],
  mode_note: "Coexistence mode: Bitfocus Companion owns USB HID.",
  key_bindings: [
    { slot: "K1", label: "Cam 1 Track", action: { type: "toggle_auto_track", camera_index: 0 } },
    { slot: "K2", label: "Cam 1 AF", action: { type: "trigger_one_push_af", camera_index: 0 } },
  ],
  dial_bindings: [],
};

describe("IntegrationsPanel (vMix, Stream Deck, Bitfocus Buttons happy & failure paths)", () => {
  it("configures vMix, maps camera tally, runs switcher commands, toggles Stream Deck, and saves Buttons API", async () => {
    vi.spyOn(api, "getVmixState").mockResolvedValue(VMIX_CONNECTED);
    vi.spyOn(api, "getStreamDeckStatus").mockResolvedValue(SD_STATUS);
    vi.spyOn(api, "getButtonsConfig").mockResolvedValue({
      enabled: false,
      base_url: "http://127.0.0.1:8000",
      api_key: "",
    });
    const updateVmix = vi.spyOn(api, "updateVmixConfig").mockResolvedValue(VMIX_CONNECTED);
    const sendCmd = vi.spyOn(api, "sendVmixCommand").mockResolvedValue(VMIX_CONNECTED);
    const bindCam = vi.spyOn(api, "updateCameraVmixBinding").mockResolvedValue({ ...CAM, vmix: { input_ref: "2" } });
    const updateSd = vi.spyOn(api, "updateStreamDeckConfig").mockResolvedValue({ ...SD_STATUS, direct_hid_enabled: true });
    const triggerSd = vi.spyOn(api, "triggerStreamDeckAction").mockResolvedValue(undefined);
    const updateButtons = vi.spyOn(api, "updateButtonsConfig").mockResolvedValue({
      enabled: true,
      base_url: "http://10.0.0.20:8000",
      api_key: "tok-123",
    });

    const onCameraUpdated = vi.fn();
    render(<IntegrationsPanel selectedCamera={CAM} onCameraUpdated={onCameraUpdated} />);

    await waitFor(() => expect(screen.getByText("vMix 28.0.0.42")).toBeInTheDocument());
    expect(screen.getByText("REC")).toBeInTheDocument();
    expect(screen.getByText("LIVE STREAM")).toBeInTheDocument();

    // Edit vMix host/ports & click Apply / Reconnect
    fireEvent.change(screen.getByPlaceholderText("127.0.0.1"), { target: { value: "10.0.0.51" } });
    fireEvent.change(screen.getByTitle("HTTP Port"), { target: { value: "8089" } });
    fireEvent.change(screen.getByTitle("TCP Tally Port"), { target: { value: "8100" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply / Reconnect" }));
    await waitFor(() =>
      expect(updateVmix).toHaveBeenCalledWith({ enabled: true, host: "10.0.0.51", http_port: 8089, tcp_port: 8100 })
    );

    // Map camera tally input
    fireEvent.change(screen.getByPlaceholderText(/Input #/i), { target: { value: "2" } });
    await waitFor(() => expect(bindCam).toHaveBeenCalledWith("cam-1", "2"));
    expect(onCameraUpdated).toHaveBeenCalled();

    // vMix switching bus + input preview click
    fireEvent.click(screen.getByRole("button", { name: "#3 Rear" }));
    fireEvent.click(screen.getByRole("button", { name: "CUT" }));
    fireEvent.click(screen.getByRole("button", { name: "FADE 500ms" }));
    fireEvent.click(screen.getByRole("button", { name: "STINGER 1" }));
    expect(sendCmd).toHaveBeenCalledWith({ function: "PreviewInput", input: "3", duration_ms: undefined });
    expect(sendCmd).toHaveBeenCalledWith({ function: "Cut", input: "3", duration_ms: undefined });
    expect(sendCmd).toHaveBeenCalledWith({ function: "Fade", input: "3", duration_ms: 500 });
    expect(sendCmd).toHaveBeenCalledWith({ function: "Stinger1", input: "3", duration_ms: undefined });

    // Stream Deck Direct USB toggle & simulated key press
    fireEvent.click(screen.getByRole("button", { name: "Direct USB: OFF" }));
    await waitFor(() => expect(updateSd).toHaveBeenCalledWith({ direct_hid_enabled: true, brightness: 80 }));
    fireEvent.click(screen.getByRole("button", { name: "K1: Cam 1 Track" }));
    expect(triggerSd).toHaveBeenCalledWith({ type: "toggle_auto_track", camera_index: 0 });

    // Bitfocus Buttons URL, key, enable toggle & save
    fireEvent.change(screen.getByDisplayValue("http://127.0.0.1:8000"), { target: { value: "http://10.0.0.20:8000" } });
    fireEvent.change(screen.getByPlaceholderText(/Bearer token/i), { target: { value: "tok-123" } });
    fireEvent.click(screen.getByRole("button", { name: "Disabled" }));
    await waitFor(() => expect(updateButtons).toHaveBeenCalledWith({
      enabled: true,
      base_url: "http://10.0.0.20:8000",
      api_key: "tok-123",
    }));
    fireEvent.click(screen.getByRole("button", { name: "Save Buttons API Settings" }));
    expect(updateButtons).toHaveBeenCalledTimes(2);
  });

  it("displays vMix disconnect error, manual input fallback, and tolerates poll & command HTTP failures", async () => {
    vi.spyOn(api, "getVmixState")
      .mockRejectedValueOnce(new Error("daemon unreachable"))
      .mockResolvedValue({
        enabled: true,
        connected: false,
        host: "127.0.0.1",
        http_port: 8088,
        tcp_port: 8099,
        recording: false,
        streaming: false,
        tally_string: "",
        inputs: [],
        last_error: "TCP connect refused",
      });
    vi.spyOn(api, "getStreamDeckStatus").mockRejectedValue(new Error("err"));
    vi.spyOn(api, "getButtonsConfig").mockRejectedValue(new Error("err"));
    vi.spyOn(api, "updateVmixConfig").mockResolvedValue({
      enabled: true,
      connected: false,
      host: "127.0.0.1",
      http_port: 8088,
      tcp_port: 8099,
      recording: false,
      streaming: false,
      tally_string: "",
      inputs: [],
      last_error: "TCP connect refused",
    });
    vi.spyOn(api, "sendVmixCommand").mockRejectedValue(new Error("HTTP 502: vMix disabled"));

    render(<IntegrationsPanel selectedCamera={null} onCameraUpdated={vi.fn()} />);

    // Manual target input shown when vMix has no discovered inputs
    const manualInput = screen.getByDisplayValue("1");
    fireEvent.change(manualInput, { target: { value: "5" } });

    // Enable vMix -> shows Connecting... + error banner
    fireEvent.click(screen.getByRole("button", { name: "Enable vMix" }));
    await waitFor(() => expect(screen.getByText("TCP connect refused")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Connecting..." })).toBeInTheDocument();

    // Failed CUT command does not crash UI
    fireEvent.click(screen.getByRole("button", { name: "CUT" }));
    expect(screen.getByText("TCP connect refused")).toBeInTheDocument();
  });
});
