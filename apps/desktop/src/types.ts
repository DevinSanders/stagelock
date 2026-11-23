export type FramingPreset = "close_up" | "waist_up" | "full_height";
export type RuleOfThirds = "center" | "left_third" | "right_third" | "auto_lead_room";
export type AutoFocusStrategy = "one_push_af" | "active_hill_climb";
export type TallyState = "off" | "preview" | "program";
export type TrackingState = "idle" | "locked" | "occluded_hold" | "searching";
export type FocusMonitorState = "sharp" | "motion_gated" | "defocused" | "focusing" | "cooldown";

export type VideoSourceConfig =
  | { kind: "test_pattern"; pattern: string; width: number; height: number; fps: number }
  | { kind: "ndi"; source_name: string; low_bandwidth: boolean }
  | { kind: "rtsp"; url: string; latency_ms: number; prefer_tcp: boolean }
  | { kind: "rtp"; address: string; port: number; codec: string }
  | { kind: "onvif"; host: string; port: number; username: string; password: string; resolved_rtsp_url?: string | null }
  | { kind: "web_rtc"; whep_url: string; bearer_token?: string | null }
  | { kind: "usb"; device_id: string; device_name: string; width: number; height: number; fps: number }
  | { kind: "deck_link"; device_index: number; connection: string; mode: string };

export type PtzConfig =
  | { protocol: "none" }
  | { protocol: "ndi" }
  | { protocol: "visca_ip_sony"; host: string; port: number }
  | { protocol: "visca_ip_raw_tcp"; host: string; port: number }
  | { protocol: "visca_ip_raw_udp"; host: string; port: number }
  | { protocol: "onvif"; host: string; port: number; username: string; password: string; profile_token?: string | null }
  | { protocol: "canon_xc"; host: string; port: number; username: string; password: string }
  | { protocol: "panasonic_aw"; host: string; port: number }
  | { protocol: "mock" };

export interface NdiOutputConfig {
  enabled: boolean;
  stream_name: string;
  groups?: string | null;
  include_color_adjustments: boolean;
}

export interface FramingConfig {
  auto_track_enabled: boolean;
  preset: FramingPreset;
  rule_of_thirds: RuleOfThirds;
  auto_focus_enabled: boolean;
  focus_strategy: AutoFocusStrategy;
  deadzone_x: number;
  deadzone_y: number;
  deadzone_zoom: number;
  max_speed: number;
  tally_damping_enabled: boolean;
}

export interface ColorAdjustments {
  white_balance_temp: number;
  white_balance_tint: number;
  exposure: number;
  contrast: number;
  saturation: number;
  sharpness: number;
  brightness: number;
}

export interface CameraPreset {
  slot: number;
  name: string;
}

export interface CameraConfig {
  id: string;
  name: string;
  enabled: boolean;
  source: VideoSourceConfig;
  ptz: PtzConfig;
  ndi_output: NdiOutputConfig;
  framing: FramingConfig;
  color: ColorAdjustments;
  vmix: { input_ref?: string | null };
  presets: CameraPreset[];
}

export interface SystemCapabilities {
  os: string;
  arch: string;
  gpu_names: string[];
  decode_backend: string;
  encode_backend: string;
  available_decode_backends: string[];
  available_encode_backends: string[];
  inference_provider: string;
  gstreamer_available: boolean;
  gstreamer_version?: string | null;
  ndi_runtime_available: boolean;
  ndi_runtime_path?: string | null;
}

export interface NormalizedRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Keypoint2D {
  x: number;
  y: number;
  score: number;
}

export interface DetectedSubject {
  track_id: number;
  bbox: NormalizedRect;
  confidence: number;
  is_locked: boolean;
  identity_similarity?: number | null;
  keypoints: Keypoint2D[];
  face_sharpness?: number | null;
}

export interface CameraTelemetry {
  camera_id: string;
  timestamp_ms: number;
  stream_status:
    | { state: "stopped" }
    | { state: "connecting" }
    | { state: "running"; width: number; height: number; fps: number; hw_accelerated: boolean; decoder_name: string }
    | { state: "reconnecting"; attempt: number; last_error: string }
    | { state: "error"; message: string };
  ndi_output_active: boolean;
  tally: TallyState;
  tracking_state: TrackingState;
  auto_track_enabled: boolean;
  locked_track_id?: number | null;
  framing_preset: FramingPreset;
  rule_of_thirds: RuleOfThirds;
  target_anchor: [number, number];
  deadzone: NormalizedRect;
  focus_state: FocusMonitorState;
  focus_sharpness: number;
  ptz_velocity: { pan: number; tilt: number; zoom: number };
  subjects: DetectedSubject[];
  preview_fps: number;
  inference_fps: number;
}

export interface NdiDiscoveredSource {
  name: string;
  url_address: string;
}

export interface CaptureDeviceInfo {
  device_id: string;
  name: string;
  api: string;
  max_width: number;
  max_height: number;
}

export interface OnvifDiscoveredDevice {
  xaddr: string;
  host: string;
  port: number;
  name: string;
}

export interface OnvifResolvedInfo {
  media_xaddr: string;
  ptz_xaddr: string;
  imaging_xaddr?: string | null;
  profile_token: string;
  video_source_token?: string | null;
  rtsp_uri?: string | null;
}

export interface ModelFileStatus {
  role: string;
  filename: string;
  license: string;
  installed: boolean;
  size_bytes?: number | null;
}

export interface VisionEngineStatus {
  models_dir: string;
  onnx_feature_compiled: boolean;
  active_execution_provider: string;
  detector_ready: boolean;
  pose_ready: boolean;
  reid_ready: boolean;
  models: ModelFileStatus[];
}

export interface CalibrationReport {
  camera_id: string;
  protocol: string;
  calibration: {
    pan_scale: number;
    tilt_scale: number;
    zoom_scale: number;
    latency_ms: number;
    min_effective_speed: number;
  };
  note: string;
}

export interface VmixInputInfo {
  key: string;
  number: number;
  kind: string;
  title: string;
  short_title: string;
  state: string;
  tally: TallyState;
}

export interface VmixRuntimeState {
  enabled: boolean;
  connected: boolean;
  host: string;
  http_port: number;
  tcp_port: number;
  version?: string | null;
  active_input?: number | null;
  preview_input?: number | null;
  recording: boolean;
  streaming: boolean;
  tally_string: string;
  inputs: VmixInputInfo[];
  last_error?: string | null;
}

export interface StreamDeckBindingInfo {
  slot: string;
  label: string;
  action: Record<string, unknown>;
}

export interface StreamDeckStatus {
  direct_hid_enabled: boolean;
  brightness: number;
  connected_devices: string[];
  mode_note: string;
  key_bindings: StreamDeckBindingInfo[];
  dial_bindings: StreamDeckBindingInfo[];
}

export interface BitfocusButtonsConfig {
  enabled: boolean;
  base_url: string;
  api_key: string;
}

export interface RemoteInfo {
  lan_access_enabled: boolean;
  http_port: number;
  osc_port: number;
  pwa_ready: boolean;
  lan_urls: string[];
  local_url: string;
}

export type DaemonWsEvent =
  | { type: "capabilities" } & SystemCapabilities
  | { type: "telemetry_batch"; cameras: CameraTelemetry[] }
  | { type: "cameras_updated" }
  | { type: "notice"; level: string; message: string };

declare global {
  interface Window {
    vcHost?: {
      daemonBaseUrl: string;
      platform: string;
    };
  }
}
