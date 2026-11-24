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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_ptz_exercises_all_controls_presets_and_imaging() {
        let drv = MockPtzDriver::default();
        assert_eq!(drv.protocol_name(), "mock");

        drv.pan_tilt_speed(0.8, -0.4).await.unwrap();
        drv.zoom_speed(0.5).await.unwrap();
        drv.store_preset(2).await.unwrap();
        let saved = drv.snapshot().await;
        assert!(saved.pan_pos > 0.0 && saved.tilt_pos < 0.0 && saved.zoom_pos > 0.0);

        drv.pan_tilt_speed(-1.0, 1.0).await.unwrap();
        drv.stop().await.unwrap();
        let stopped = drv.snapshot().await;
        assert_eq!((stopped.pan_speed, stopped.tilt_speed, stopped.zoom_speed), (0.0, 0.0, 0.0));

        drv.recall_preset(2).await.unwrap();
        let recalled = drv.snapshot().await;
        assert!((recalled.pan_pos - saved.pan_pos).abs() < 1e-6);

        // Missing preset slot is a safe no-op
        drv.recall_preset(99).await.unwrap();

        for cmd in [
            FocusCommand::Manual,
            FocusCommand::Auto,
            FocusCommand::OnePush,
            FocusCommand::Near { speed: 0.5 },
            FocusCommand::Far { speed: 0.5 },
            FocusCommand::Stop,
        ] {
            drv.focus(cmd).await.unwrap();
        }
        assert!(drv.snapshot().await.auto_focus);

        for cmd in [
            ImagingCommand::WhiteBalanceIndoor,
            ImagingCommand::WhiteBalanceOutdoor,
            ImagingCommand::WhiteBalanceOnePush,
            ImagingCommand::WhiteBalanceManual { red: 0.5, blue: 0.6 },
            ImagingCommand::WhiteBalanceAuto,
            ImagingCommand::ExposureManual { level: 0.4 },
            ImagingCommand::ExposureAuto,
            ImagingCommand::BacklightComp { enabled: true },
            ImagingCommand::Sharpness { level: 0.8 },
            ImagingCommand::Contrast { level: 0.6 },
            ImagingCommand::Brightness { level: 0.5 },
        ] {
            drv.set_imaging(cmd).await.unwrap();
        }
        let s = drv.snapshot().await;
        assert_eq!(s.wb_mode, "auto");
        assert!(s.exposure_auto);
        assert!(s.backlight_comp);
        assert!((s.hardware_sharpness - 0.8).abs() < 1e-6);
    }
}
