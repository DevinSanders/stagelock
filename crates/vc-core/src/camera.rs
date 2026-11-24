use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type CameraId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VideoSourceConfig {
    /// Synthetic test pattern (useful for verification and offline camera setup)
    TestPattern {
        #[serde(default = "default_pattern")]
        pattern: String,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
        #[serde(default = "default_fps")]
        fps: u32,
    },
    /// NDI or NDI|HX network stream (discovered via NDI 6 runtime)
    Ndi {
        source_name: String,
        #[serde(default)]
        low_bandwidth: bool,
    },
    /// RTSP IP camera stream
    Rtsp {
        url: String,
        #[serde(default = "default_rtsp_latency")]
        latency_ms: u32,
        #[serde(default = "default_true")]
        prefer_tcp: bool,
    },
    /// Raw RTP stream
    Rtp {
        address: String,
        port: u16,
        #[serde(default = "default_rtp_codec")]
        codec: String,
    },
    /// ONVIF camera (Profile S/T)
    Onvif {
        host: String,
        #[serde(default = "default_onvif_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        resolved_rtsp_url: Option<String>,
    },
    /// WebRTC (WHEP) ingest stream
    WebRtc {
        whep_url: String,
        #[serde(default)]
        bearer_token: Option<String>,
    },
    /// USB UVC webcam or USB HDMI capture dongle
    Usb {
        device_id: String,
        device_name: String,
        #[serde(default = "default_width")]
        width: u32,
        #[serde(default = "default_height")]
        height: u32,
        #[serde(default = "default_fps")]
        fps: u32,
    },
    /// Blackmagic DeckLink / UltraStudio SDI or HDMI capture card
    DeckLink {
        device_index: u32,
        #[serde(default = "default_decklink_connection")]
        connection: String,
        #[serde(default = "default_decklink_mode")]
        mode: String,
    },
}

fn default_pattern() -> String {
    "smpte".to_string()
}
fn default_width() -> u32 {
    1280
}
fn default_height() -> u32 {
    720
}
fn default_fps() -> u32 {
    30
}
fn default_rtsp_latency() -> u32 {
    25
}
fn default_true() -> bool {
    true
}
fn default_rtp_codec() -> String {
    "H264".to_string()
}
fn default_onvif_port() -> u16 {
    80
}
fn default_decklink_connection() -> String {
    "auto".to_string()
}
fn default_decklink_mode() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum PtzConfig {
    None,
    /// Native NDI PTZ control bound to the NDI video receiver
    Ndi,
    /// Sony VISCA-over-IP (UDP with 8-byte sequence header, default port 52381)
    ViscaIpSony {
        host: String,
        #[serde(default = "default_sony_visca_port")]
        port: u16,
    },
    /// Raw VISCA over TCP (PTZOptics default 5678, BirdDog, Marshall, Lumens)
    ViscaIpRawTcp {
        host: String,
        #[serde(default = "default_raw_visca_tcp_port")]
        port: u16,
    },
    /// Raw VISCA over UDP (PTZOptics UDP 1259, Aver, etc.)
    ViscaIpRawUdp {
        host: String,
        #[serde(default = "default_raw_visca_udp_port")]
        port: u16,
    },
    /// ONVIF Profile S/T PTZ + Imaging control
    Onvif {
        host: String,
        #[serde(default = "default_onvif_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        profile_token: Option<String>,
    },
    /// Canon XC HTTP/CGI protocol
    CanonXc {
        host: String,
        #[serde(default = "default_onvif_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    /// Panasonic AW HTTP/CGI protocol
    PanasonicAw {
        host: String,
        #[serde(default = "default_onvif_port")]
        port: u16,
    },
    /// Software physics-simulated PTZ camera for testing
    Mock,
}

impl Default for PtzConfig {
    fn default() -> Self {
        Self::None
    }
}

fn default_sony_visca_port() -> u16 {
    52381
}
fn default_raw_visca_tcp_port() -> u16 {
    5678
}
fn default_raw_visca_udp_port() -> u16 {
    1259
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NdiOutputConfig {
    pub enabled: bool,
    pub stream_name: String,
    #[serde(default)]
    pub groups: Option<String>,
    #[serde(default = "default_true")]
    pub include_color_adjustments: bool,
}

impl Default for NdiOutputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stream_name: "VideoController Output".to_string(),
            groups: None,
            include_color_adjustments: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FramingPreset {
    CloseUp,
    #[default]
    WaistUp,
    FullHeight,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleOfThirds {
    #[default]
    Center,
    LeftThird,
    RightThird,
    AutoLeadRoom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoFocusStrategy {
    #[default]
    OnePushAf,
    ActiveHillClimb,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FramingConfig {
    #[serde(default)]
    pub auto_track_enabled: bool,
    #[serde(default)]
    pub preset: FramingPreset,
    #[serde(default)]
    pub rule_of_thirds: RuleOfThirds,
    #[serde(default = "default_true")]
    pub auto_focus_enabled: bool,
    #[serde(default)]
    pub focus_strategy: AutoFocusStrategy,
    /// Outer horizontal dead-zone half-width (normalized 0.0..0.5)
    #[serde(default = "default_deadzone_x")]
    pub deadzone_x: f32,
    /// Outer vertical dead-zone half-height (normalized 0.0..0.5)
    #[serde(default = "default_deadzone_y")]
    pub deadzone_y: f32,
    /// Zoom tolerance before issuing smooth zoom corrections
    #[serde(default = "default_deadzone_zoom")]
    pub deadzone_zoom: f32,
    /// Maximum PTZ tracking speed scalar (0.1..1.0)
    #[serde(default = "default_tracking_speed")]
    pub max_speed: f32,
    /// Dampen movement velocity automatically when camera is Live on vMix Program
    #[serde(default = "default_true")]
    pub tally_damping_enabled: bool,
}

fn default_deadzone_x() -> f32 {
    0.08
}
fn default_deadzone_y() -> f32 {
    0.06
}
fn default_deadzone_zoom() -> f32 {
    0.12
}
fn default_tracking_speed() -> f32 {
    0.65
}

impl Default for FramingConfig {
    fn default() -> Self {
        Self {
            auto_track_enabled: false,
            preset: FramingPreset::WaistUp,
            rule_of_thirds: RuleOfThirds::Center,
            auto_focus_enabled: true,
            focus_strategy: AutoFocusStrategy::OnePushAf,
            deadzone_x: default_deadzone_x(),
            deadzone_y: default_deadzone_y(),
            deadzone_zoom: default_deadzone_zoom(),
            max_speed: default_tracking_speed(),
            tally_damping_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorAdjustments {
    /// White balance temperature shift (-1.0 cool .. 1.0 warm)
    #[serde(default)]
    pub white_balance_temp: f32,
    /// White balance tint shift (-1.0 green .. 1.0 magenta)
    #[serde(default)]
    pub white_balance_tint: f32,
    /// Exposure stops offset (-2.0 .. 2.0)
    #[serde(default)]
    pub exposure: f32,
    /// Contrast multiplier (0.0 .. 2.0, default 1.0)
    #[serde(default = "default_one")]
    pub contrast: f32,
    /// Saturation multiplier (0.0 .. 2.0, default 1.0)
    #[serde(default = "default_one")]
    pub saturation: f32,
    /// Sharpness boost (0.0 .. 2.0, default 0.0)
    #[serde(default)]
    pub sharpness: f32,
    /// Brightness offset (-1.0 .. 1.0, default 0.0)
    #[serde(default)]
    pub brightness: f32,
}

fn default_one() -> f32 {
    1.0
}

impl Default for ColorAdjustments {
    fn default() -> Self {
        Self {
            white_balance_temp: 0.0,
            white_balance_tint: 0.0,
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            sharpness: 0.0,
            brightness: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VmixBinding {
    /// vMix input number, title, or GUID associated with this camera
    #[serde(default)]
    pub input_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraPreset {
    pub slot: u8,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraCalibration {
    /// Gain scalar calibrated for this camera's horizontal pan motor response
    #[serde(default = "default_one")]
    pub pan_scale: f32,
    /// Gain scalar calibrated for this camera's vertical tilt motor response
    #[serde(default = "default_one")]
    pub tilt_scale: f32,
    /// Gain scalar calibrated for this camera's optical zoom motor response
    #[serde(default = "default_one")]
    pub zoom_scale: f32,
    /// Measured end-to-end video + network + motor latency (ms)
    #[serde(default = "default_calibration_latency_ms")]
    pub latency_ms: f32,
    /// Minimum normalized motor speed below which camera stays stationary (motor deadband)
    #[serde(default = "default_min_effective_speed")]
    pub min_effective_speed: f32,
}

fn default_calibration_latency_ms() -> f32 {
    180.0
}
fn default_min_effective_speed() -> f32 {
    0.03
}

impl Default for CameraCalibration {
    fn default() -> Self {
        Self {
            pan_scale: 1.0,
            tilt_scale: 1.0,
            zoom_scale: 1.0,
            latency_ms: default_calibration_latency_ms(),
            min_effective_speed: default_min_effective_speed(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraConfig {
    pub id: CameraId,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: VideoSourceConfig,
    #[serde(default)]
    pub ptz: PtzConfig,
    #[serde(default)]
    pub ndi_output: NdiOutputConfig,
    #[serde(default)]
    pub framing: FramingConfig,
    #[serde(default)]
    pub color: ColorAdjustments,
    #[serde(default)]
    pub vmix: VmixBinding,
    #[serde(default)]
    pub presets: Vec<CameraPreset>,
    #[serde(default)]
    pub calibration: CameraCalibration,
}

impl CameraConfig {
    pub fn new_demo(name: impl Into<String>, pattern: impl Into<String>) -> Self {
        let name = name.into();
        let id = Uuid::new_v4();
        Self {
            id,
            ndi_output: NdiOutputConfig {
                enabled: false,
                stream_name: format!("VC - {name}"),
                groups: None,
                include_color_adjustments: true,
            },
            name,
            enabled: true,
            source: VideoSourceConfig::TestPattern {
                pattern: pattern.into(),
                width: 1280,
                height: 720,
                fps: 30,
            },
            ptz: PtzConfig::Mock,
            framing: FramingConfig::default(),
            color: ColorAdjustments::default(),
            vmix: VmixBinding::default(),
            presets: vec![
                CameraPreset { slot: 1, name: "Wide Stage".into() },
                CameraPreset { slot: 2, name: "Pulpit / Center".into() },
                CameraPreset { slot: 3, name: "Stage Left".into() },
                CameraPreset { slot: 4, name: "Stage Right".into() },
            ],
            calibration: CameraCalibration::default(),
        }
    }
}
