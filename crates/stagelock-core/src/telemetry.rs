use serde::{Deserialize, Serialize};
use crate::camera::{CameraId, FramingPreset, RuleOfThirds};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HardwareAccelBackend {
    NvidiaNvcodec,
    AmdAmf,
    IntelQuickSync,
    WindowsD3d11,
    AppleVideoToolbox,
    LinuxVaapi,
    SoftwareCpu,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceProvider {
    TensorRt,
    Cuda,
    DirectMl,
    OpenVino,
    CoreMl,
    Rocm,
    Cpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemCapabilities {
    pub os: String,
    pub arch: String,
    pub gpu_names: Vec<String>,
    pub decode_backend: HardwareAccelBackend,
    pub encode_backend: HardwareAccelBackend,
    pub available_decode_backends: Vec<HardwareAccelBackend>,
    pub available_encode_backends: Vec<HardwareAccelBackend>,
    pub inference_provider: InferenceProvider,
    pub gstreamer_available: bool,
    pub gstreamer_version: Option<String>,
    pub ndi_runtime_available: bool,
    pub ndi_runtime_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CameraStreamStatus {
    Stopped,
    Connecting,
    Running {
        width: u32,
        height: u32,
        fps: f32,
        hw_accelerated: bool,
        decoder_name: String,
    },
    Reconnecting {
        attempt: u32,
        last_error: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TallyState {
    #[default]
    Off,
    Preview,
    Program,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackingState {
    #[default]
    Idle,
    Locked,
    OccludedHold,
    Searching,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FocusMonitorState {
    #[default]
    Sharp,
    MotionGated,
    Defocused,
    Focusing,
    Cooldown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Keypoint2D {
    pub x: f32,
    pub y: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectedSubject {
    pub track_id: u64,
    pub bbox: NormalizedRect,
    pub confidence: f32,
    pub is_locked: bool,
    pub identity_similarity: Option<f32>,
    pub keypoints: Vec<Keypoint2D>,
    pub face_sharpness: Option<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct PtzVelocity {
    pub pan: f32,
    pub tilt: f32,
    pub zoom: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraTelemetry {
    pub camera_id: CameraId,
    pub timestamp_ms: i64,
    pub stream_status: CameraStreamStatus,
    pub ndi_output_active: bool,
    pub tally: TallyState,
    pub tracking_state: TrackingState,
    pub auto_track_enabled: bool,
    pub locked_track_id: Option<u64>,
    pub framing_preset: FramingPreset,
    pub rule_of_thirds: RuleOfThirds,
    pub target_anchor: [f32; 2],
    pub deadzone: NormalizedRect,
    pub focus_state: FocusMonitorState,
    pub focus_sharpness: f32,
    pub ptz_velocity: PtzVelocity,
    pub subjects: Vec<DetectedSubject>,
    pub preview_fps: f32,
    pub inference_fps: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonWsEvent {
    Capabilities(SystemCapabilities),
    TelemetryBatch {
        cameras: Vec<CameraTelemetry>,
    },
    CamerasUpdated,
    Notice {
        level: String,
        message: String,
    },
}
