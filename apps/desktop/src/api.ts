import type {
  BitfocusButtonsConfig,
  CalibrationReport,
  CameraConfig,
  CaptureDeviceInfo,
  ColorAdjustments,
  FramingConfig,
  NdiDiscoveredSource,
  NdiOutputConfig,
  OnvifDiscoveredDevice,
  OnvifResolvedInfo,
  RemoteInfo,
  StreamDeckStatus,
  SystemCapabilities,
  VisionEngineStatus,
  VmixRuntimeState,
} from "./types";

function resolveDaemonUrl(): string {
  if (window.vcHost?.daemonBaseUrl) {
    return window.vcHost.daemonBaseUrl;
  }
  const { protocol, hostname, port, origin } = window.location;
  // When running Vite dev server (:5173) from desktop or a LAN phone, target daemon on :8765
  if (port === "5173") {
    return `${protocol}//${hostname}:8765`;
  }
  return origin;
}

export const DAEMON_URL = resolveDaemonUrl();
export const IS_ELECTRON = Boolean(window.vcHost);

export function mjpegStreamUrl(cameraId: string, maxFps?: number): string {
  const qs = maxFps ? `?max_fps=${maxFps}` : "";
  return `${DAEMON_URL}/api/v1/cameras/${cameraId}/stream.mjpeg${qs}`;
}

export function telemetryWsUrl(): string {
  return DAEMON_URL.replace(/^http/, "ws") + "/ws/telemetry";
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${DAEMON_URL}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export const api = {
  getCapabilities: () => req<SystemCapabilities>("/api/v1/capabilities"),
  getVisionStatus: () => req<VisionEngineStatus>("/api/v1/vision/status"),
  getRemoteInfo: () => req<RemoteInfo>("/api/v1/remote-info"),
  discoverNdi: () => req<{ available: boolean; sources: NdiDiscoveredSource[] }>("/api/v1/discovery/ndi"),
  discoverUsb: () => req<{ devices: CaptureDeviceInfo[] }>("/api/v1/discovery/usb"),
  discoverOnvif: () => req<{ devices: OnvifDiscoveredDevice[] }>("/api/v1/discovery/onvif"),
  resolveOnvif: (body: { host: string; port: number; username: string; password: string }) =>
    req<OnvifResolvedInfo>("/api/v1/onvif/resolve", { method: "POST", body: JSON.stringify(body) }),
  listCameras: () => req<CameraConfig[]>("/api/v1/cameras"),
  createCamera: (cam: CameraConfig) =>
    req<CameraConfig>("/api/v1/cameras", { method: "POST", body: JSON.stringify(cam) }),
  deleteCamera: (id: string) => req<void>(`/api/v1/cameras/${id}`, { method: "DELETE" }),
  updateNdiOutput: (id: string, ndi: NdiOutputConfig) =>
    req<CameraConfig>(`/api/v1/cameras/${id}/ndi-output`, { method: "PUT", body: JSON.stringify(ndi) }),
  updateFraming: (id: string, framing: FramingConfig) =>
    req<CameraConfig>(`/api/v1/cameras/${id}/framing`, { method: "PUT", body: JSON.stringify(framing) }),
  updateColor: (id: string, color: ColorAdjustments) =>
    req<CameraConfig>(`/api/v1/cameras/${id}/color`, { method: "PUT", body: JSON.stringify(color) }),
  cameraImaging: (id: string, cmd: Record<string, unknown>) =>
    req<void>(`/api/v1/cameras/${id}/imaging`, { method: "POST", body: JSON.stringify(cmd) }),
  selectSubject: (id: string, body: { track_id?: number; norm_x?: number; norm_y?: number; clear?: boolean }) =>
    req<{ locked_track_id: number | null }>(`/api/v1/cameras/${id}/tracking/select`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  ptzMove: (id: string, pan: number, tilt: number, zoom = 0) =>
    req<void>(`/api/v1/cameras/${id}/ptz/move`, { method: "POST", body: JSON.stringify({ pan, tilt, zoom }) }),
  ptzStop: (id: string) => req<void>(`/api/v1/cameras/${id}/ptz/stop`, { method: "POST" }),
  ptzFocus: (id: string, cmd: Record<string, unknown>) =>
    req<void>(`/api/v1/cameras/${id}/ptz/focus`, { method: "POST", body: JSON.stringify(cmd) }),
  ptzPreset: (id: string, action: "recall" | "store", slot: number) =>
    req<void>(`/api/v1/cameras/${id}/ptz/preset`, { method: "POST", body: JSON.stringify({ action, slot }) }),
  ptzCalibrate: (id: string) =>
    req<CalibrationReport>(`/api/v1/cameras/${id}/ptz/calibrate`, { method: "POST" }),
  updateCameraVmixBinding: (id: string, input_ref: string | null) =>
    req<CameraConfig>(`/api/v1/cameras/${id}/vmix`, {
      method: "PUT",
      body: JSON.stringify({ input_ref }),
    }),
  getVmixState: () => req<VmixRuntimeState>("/api/v1/integrations/vmix"),
  updateVmixConfig: (cfg: { enabled: boolean; host: string; http_port: number; tcp_port: number }) =>
    req<VmixRuntimeState>("/api/v1/integrations/vmix/config", { method: "PUT", body: JSON.stringify(cfg) }),
  sendVmixCommand: (cmd: { function: string; input?: string; value?: string; duration_ms?: number }) =>
    req<VmixRuntimeState>("/api/v1/integrations/vmix/command", { method: "POST", body: JSON.stringify(cmd) }),
  getStreamDeckStatus: () => req<StreamDeckStatus>("/api/v1/integrations/streamdeck"),
  updateStreamDeckConfig: (cfg: { direct_hid_enabled: boolean; brightness: number }) =>
    req<StreamDeckStatus>("/api/v1/integrations/streamdeck/config", { method: "PUT", body: JSON.stringify(cfg) }),
  triggerStreamDeckAction: (action: Record<string, unknown>) =>
    req<void>("/api/v1/integrations/streamdeck/trigger", { method: "POST", body: JSON.stringify(action) }),
  getButtonsConfig: () => req<BitfocusButtonsConfig>("/api/v1/integrations/buttons/config"),
  updateButtonsConfig: (cfg: BitfocusButtonsConfig) =>
    req<BitfocusButtonsConfig>("/api/v1/integrations/buttons/config", { method: "PUT", body: JSON.stringify(cfg) }),
};
