import { afterEach, describe, expect, it, vi } from "vitest";
import { api, mjpegStreamUrl, telemetryWsUrl, videoH264WsUrl } from "./api";
import type { CameraConfig } from "./types";

describe("api client (happy paths & HTTP failure paths)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("constructs MJPEG, H.264 video WS, and telemetry WS URLs", () => {
    expect(mjpegStreamUrl("cam-1")).toContain("/api/v1/cameras/cam-1/stream.mjpeg");
    expect(mjpegStreamUrl("cam-1", 12)).toContain("?max_fps=12");
    expect(videoH264WsUrl("cam-1")).toMatch(/^ws:\/\/.*\/ws\/video\/cam-1$/);
    expect(telemetryWsUrl()).toMatch(/^ws:\/\/.*\/ws\/telemetry$/);
  });

  it("executes all REST endpoints cleanly on 200/204 responses", async () => {
    const fetchMock = vi.fn().mockImplementation((url: string, init?: RequestInit) => {
      if (init?.method === "DELETE" || url.includes("/ptz/move") || url.includes("/ptz/stop") || url.includes("/ptz/focus") || url.includes("/ptz/preset") || url.includes("/imaging") || url.includes("/streamdeck/trigger")) {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    });
    vi.stubGlobal("fetch", fetchMock);

    await api.getCapabilities();
    await api.getVisionStatus();
    await api.getRemoteInfo();
    await api.discoverNdi();
    await api.discoverUsb();
    await api.discoverOnvif();
    await api.resolveOnvif({ host: "10.0.0.5", port: 80, username: "a", password: "b" });
    await api.listCameras();
    await api.createCamera({ id: "1", name: "C1" } as unknown as CameraConfig);
    await api.deleteCamera("1");
    await api.updateNdiOutput("1", { enabled: true, stream_name: "Out", include_color_adjustments: true });
    await api.updateFraming("1", { auto_track_enabled: true } as CameraConfig["framing"]);
    await api.updateColor("1", { contrast: 1.1 } as CameraConfig["color"]);
    await api.cameraImaging("1", { command: "white_balance_auto" });
    await api.selectSubject("1", { track_id: 2 });
    await api.ptzMove("1", 0.5, -0.2, 0.1);
    await api.ptzMove("1", 0.1, 0.2);
    await api.ptzStop("1");
    await api.ptzFocus("1", { mode: "one_push" });
    await api.ptzPreset("1", "recall", 1);
    await api.ptzCalibrate("1");
    await api.updateCameraVmixBinding("1", "2");
    await api.getVmixState();
    await api.updateVmixConfig({ enabled: true, host: "127.0.0.1", http_port: 8088, tcp_port: 8099 });
    await api.sendVmixCommand({ function: "Cut", input: "1" });
    await api.getStreamDeckStatus();
    await api.updateStreamDeckConfig({ direct_hid_enabled: false, brightness: 80 });
    await api.triggerStreamDeckAction({ type: "toggle_auto_track", camera_index: 0 });
    await api.getButtonsConfig();
    await api.updateButtonsConfig({ enabled: false, base_url: "http://127.0.0.1:8000", api_key: "" });

    expect(fetchMock).toHaveBeenCalledTimes(30);
  });

  it("throws informative Error on HTTP 404 / 502 and network failures", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(new Response("Camera not found", { status: 404 }))
        .mockResolvedValueOnce(new Response("vMix disabled", { status: 502 }))
        .mockRejectedValueOnce(new TypeError("Failed to fetch"))
    );

    await expect(api.deleteCamera("missing-id")).rejects.toThrow("HTTP 404: Camera not found");
    await expect(api.sendVmixCommand({ function: "Cut" })).rejects.toThrow("HTTP 502: vMix disabled");
    await expect(api.listCameras()).rejects.toThrow("Failed to fetch");
  });

  it("resolves daemon URL across host bridge, Tauri, Vite :5173, and file: origins", async () => {
    // Host bridge
    (window as unknown as { vcHost?: { daemonBaseUrl: string } }).vcHost = {
      daemonBaseUrl: "http://10.0.0.9:8765",
    };
    vi.resetModules();
    let mod = await import("./api");
    expect(mod.DAEMON_URL).toBe("http://10.0.0.9:8765");
    expect(mod.IS_DESKTOP).toBe(true);
    delete (window as unknown as { vcHost?: unknown }).vcHost;

    // Tauri 2 globals
    (window as unknown as { __TAURI_INTERNALS__?: object }).__TAURI_INTERNALS__ = {};
    vi.resetModules();
    mod = await import("./api");
    expect(mod.DAEMON_URL).toBe("http://127.0.0.1:8765");
    expect(mod.IS_DESKTOP).toBe(true);
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;

    // Vite dev server on :5173
    const origLocation = window.location;
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { protocol: "http:", hostname: "192.168.1.20", port: "5173", origin: "http://192.168.1.20:5173" },
    });
    vi.resetModules();
    mod = await import("./api");
    expect(mod.DAEMON_URL).toBe("http://192.168.1.20:8765");

    // Packaged file: fallback
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { protocol: "file:", hostname: "", port: "", origin: "null" },
    });
    vi.resetModules();
    mod = await import("./api");
    expect(mod.DAEMON_URL).toBe("http://127.0.0.1:8765");

    Object.defineProperty(window, "location", { configurable: true, value: origLocation });
  });
});
