use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FocusCommand {
    Auto,
    Manual,
    OnePush,
    Near { speed: f32 },
    Far { speed: f32 },
    Stop,
}

/// Layer-1 camera hardware imaging controls sent directly to the camera sensor/ISP
/// via VISCA, ONVIF Imaging Service, NDI PTZ, Canon XC, or Panasonic AW.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "control", rename_all = "snake_case")]
pub enum ImagingCommand {
    WhiteBalanceAuto,
    WhiteBalanceIndoor,
    WhiteBalanceOutdoor,
    WhiteBalanceOnePush,
    WhiteBalanceManual { red: f32, blue: f32 },
    ExposureAuto,
    ExposureManual { level: f32 },
    BacklightComp { enabled: bool },
    Sharpness { level: f32 },
    Contrast { level: f32 },
    Brightness { level: f32 },
}

#[async_trait]
pub trait PtzDriver: Send + Sync {
    fn protocol_name(&self) -> &'static str;

    /// Drive pan (-1.0 left .. 1.0 right) and tilt (-1.0 down .. 1.0 up) smoothly.
    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()>;

    /// Drive optical zoom (-1.0 wide .. 1.0 tele).
    async fn zoom_speed(&self, zoom: f32) -> Result<()>;

    /// Halt all pan, tilt, and zoom motion immediately.
    async fn stop(&self) -> Result<()>;

    /// Control camera focus (auto, manual, one-push AF, near/far step, stop).
    async fn focus(&self, cmd: FocusCommand) -> Result<()>;

    /// Recall a stored camera preset slot (0..=127).
    async fn recall_preset(&self, slot: u8) -> Result<()>;

    /// Store current camera position into a preset slot (0..=127).
    async fn store_preset(&self, slot: u8) -> Result<()>;

    /// Adjust Layer-1 camera hardware imaging (WB, exposure, sharpness, contrast, etc.).
    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()>;
}
