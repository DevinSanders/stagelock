import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { SetupWizardModal } from "./SetupWizardModal";
import type { SystemCapabilities, VisionEngineStatus } from "../types";

const CAPS_OK: SystemCapabilities = {
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
  ndi_runtime_path: "C:\\Program Files\\NDI\\Processing.NDI.Lib.x64.dll",
};

const VISION: VisionEngineStatus = {
  onnx_feature_compiled: true,
  active_execution_provider: "CUDA",
  models_dir: "models",
  detector_ready: true,
  pose_ready: false,
  reid_ready: false,
  models: [
    { role: "detector", filename: "rtmdet_s.onnx", installed: true, license: "Apache-2.0" },
    { role: "pose", filename: "rtmpose_s.onnx", installed: false, license: "Apache-2.0" },
  ],
};

describe("SetupWizardModal (diagnostics, source discovery happy & failure paths)", () => {
  it("displays GPU/runtime diagnostics, runs full NDI+ONVIF+USB scan, and opens Add Camera wizard", async () => {
    vi.spyOn(api, "getRemoteInfo").mockResolvedValue({
      lan_access_enabled: true,
      http_port: 8765,
      osc_port: 9000,
      pwa_ready: true,
      lan_urls: ["http://10.7.21.2:8765"],
      local_url: "http://127.0.0.1:8765",
    });
    vi.spyOn(api, "discoverNdi").mockResolvedValue({
      available: true,
      sources: [{ name: "STUDIO (CAM 1)", url_address: "10.0.0.10:5961" }],
    });
    vi.spyOn(api, "discoverOnvif").mockResolvedValue({
      devices: [{ xaddr: "http://10.0.0.11/onvif", host: "10.0.0.11", port: 80, name: "PTZOptics" }],
    });
    vi.spyOn(api, "discoverUsb").mockResolvedValue({
      devices: [
        { device_id: "0", name: "USB HDMI 1", api: "mf", max_width: 1920, max_height: 1080 },
        { device_id: "1", name: "USB HDMI 2", api: "mf", max_width: 1920, max_height: 1080 },
        { device_id: "2", name: "USB HDMI 3", api: "mf", max_width: 1920, max_height: 1080 },
        { device_id: "3", name: "USB HDMI 4", api: "mf", max_width: 1920, max_height: 1080 },
      ],
    });

    const onClose = vi.fn();
    const onOpenAdd = vi.fn();
    render(
      <SetupWizardModal capabilities={CAPS_OK} visionStatus={VISION} onClose={onClose} onOpenAddCamera={onOpenAdd} />
    );

    expect(screen.getByText("NVIDIA GeForce RTX 5080")).toBeInTheDocument();
    expect(screen.getByText(/rtmdet_s\.onnx: Installed/i)).toBeInTheDocument();
    expect(screen.getByText(/rtmpose_s\.onnx: Fallback Mode/i)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText(/Remote PWA Ready: http:\/\/10\.7\.21\.2:8765/)).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: /Scan NDI \+ ONVIF \+ USB/i }));
    await waitFor(() => expect(screen.getByText(/1 found \(STUDIO \(CAM 1\)\)/)).toBeInTheDocument());
    expect(screen.getByText(/4 found \(USB HDMI 1, USB HDMI 2, USB HDMI 3, \.\.\.\)/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /\+ Open Add Camera Wizard/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenAdd).toHaveBeenCalledTimes(1);
  });

  it("shows missing NDI runtime warning and recovers cleanly when all discovery endpoints fail", async () => {
    vi.spyOn(api, "getRemoteInfo").mockRejectedValue(new Error("offline"));
    vi.spyOn(api, "discoverNdi").mockRejectedValue(new Error("ndi err"));
    vi.spyOn(api, "discoverOnvif").mockRejectedValue(new Error("onvif err"));
    vi.spyOn(api, "discoverUsb").mockRejectedValue(new Error("usb err"));

    render(
      <SetupWizardModal
        capabilities={{ ...CAPS_OK, gstreamer_available: false, gstreamer_version: undefined, ndi_runtime_available: false }}
        visionStatus={null}
        onClose={vi.fn()}
        onOpenAddCamera={vi.fn()}
      />
    );

    expect(screen.getByText("Missing")).toBeInTheDocument();
    expect(screen.getByText(/Install the free NDI® Runtime/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Scan NDI \+ ONVIF \+ USB/i }));
    await waitFor(() => expect(screen.getAllByText(/0 found/)).toHaveLength(3));
  });
});
