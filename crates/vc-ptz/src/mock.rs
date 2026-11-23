use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::driver::{FocusCommand, ImagingCommand, PtzDriver};

#[derive(Debug, Clone, Default)]
pub struct MockPtzState {
    pub pan_speed: f32,
    pub tilt_speed: f32,
    pub zoom_speed: f32,
    pub pan_pos: f32,
    pub tilt_pos: f32,
    pub zoom_pos: f32,
    pub auto_focus: bool,
    pub wb_mode: String,
    pub exposure_auto: bool,
    pub hardware_sharpness: f32,
    pub backlight_comp: bool,
    pub presets: HashMap<u8, (f32, f32, f32)>,
}

/// Deterministic physics-simulated PTZ camera for testing UI controls and closed-loop tracking.
pub struct MockPtzDriver {
    state: Arc<RwLock<MockPtzState>>,
}

impl MockPtzDriver {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(MockPtzState {
                auto_focus: true,
                wb_mode: "auto".into(),
                exposure_auto: true,
                hardware_sharpness: 0.5,
                ..Default::default()
            })),
        }
    }

    pub async fn snapshot(&self) -> MockPtzState {
        self.state.read().await.clone()
    }
}

impl Default for MockPtzDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PtzDriver for MockPtzDriver {
    fn protocol_name(&self) -> &'static str {
        "mock"
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        let mut s = self.state.write().await;
        s.pan_speed = pan.clamp(-1.0, 1.0);
        s.tilt_speed = tilt.clamp(-1.0, 1.0);
        s.pan_pos = (s.pan_pos + s.pan_speed * 0.05).clamp(-1.0, 1.0);
        s.tilt_pos = (s.tilt_pos + s.tilt_speed * 0.05).clamp(-1.0, 1.0);
        debug!(pan = s.pan_speed, tilt = s.tilt_speed, "Mock PTZ pan/tilt");
        Ok(())
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        let mut s = self.state.write().await;
        s.zoom_speed = zoom.clamp(-1.0, 1.0);
        s.zoom_pos = (s.zoom_pos + s.zoom_speed * 0.05).clamp(0.0, 1.0);
        debug!(zoom = s.zoom_speed, "Mock PTZ zoom");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut s = self.state.write().await;
        s.pan_speed = 0.0;
        s.tilt_speed = 0.0;
        s.zoom_speed = 0.0;
        Ok(())
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        let mut s = self.state.write().await;
        match cmd {
            FocusCommand::Auto => s.auto_focus = true,
            FocusCommand::Manual => s.auto_focus = false,
            FocusCommand::OnePush => debug!("Mock PTZ one-push auto-focus triggered"),
            FocusCommand::Near { .. } | FocusCommand::Far { .. } | FocusCommand::Stop => {}
        }
        Ok(())
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        let mut s = self.state.write().await;
        if let Some(&(p, t, z)) = s.presets.get(&slot) {
            s.pan_pos = p;
            s.tilt_pos = t;
            s.zoom_pos = z;
        }
        debug!(slot, "Mock PTZ recalled preset");
        Ok(())
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        let mut s = self.state.write().await;
        let pos = (s.pan_pos, s.tilt_pos, s.zoom_pos);
        s.presets.insert(slot, pos);
        debug!(slot, "Mock PTZ stored preset");
        Ok(())
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        let mut s = self.state.write().await;
        match cmd {
            ImagingCommand::WhiteBalanceAuto => s.wb_mode = "auto".into(),
            ImagingCommand::WhiteBalanceIndoor => s.wb_mode = "indoor".into(),
            ImagingCommand::WhiteBalanceOutdoor => s.wb_mode = "outdoor".into(),
            ImagingCommand::WhiteBalanceOnePush => s.wb_mode = "one_push".into(),
            ImagingCommand::WhiteBalanceManual { .. } => s.wb_mode = "manual".into(),
            ImagingCommand::ExposureAuto => s.exposure_auto = true,
            ImagingCommand::ExposureManual { .. } => s.exposure_auto = false,
            ImagingCommand::BacklightComp { enabled } => s.backlight_comp = enabled,
            ImagingCommand::Sharpness { level } => s.hardware_sharpness = level.clamp(0.0, 1.0),
            ImagingCommand::Contrast { .. } | ImagingCommand::Brightness { .. } => {}
        }
        debug!(?cmd, "Mock PTZ imaging updated");
        Ok(())
    }
}
