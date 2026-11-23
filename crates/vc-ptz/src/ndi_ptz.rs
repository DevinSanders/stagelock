//! Native NDI® PTZ + Camera Imaging driver bound to the active `vc-media` NDI 6 receiver.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use vc_core::CameraId;
use vc_media::{MediaEngine, NdiReceiver};

use crate::driver::{FocusCommand, ImagingCommand, PtzDriver};

pub struct NdiPtzDriver {
    camera_id: CameraId,
    media: MediaEngine,
}

impl NdiPtzDriver {
    pub fn new(camera_id: CameraId, media: MediaEngine) -> Self {
        Self { camera_id, media }
    }

    async fn active_receiver(&self) -> Result<Arc<NdiReceiver>> {
        let handle = self
            .media
            .get_handle(self.camera_id)
            .await
            .ok_or_else(|| anyhow!("NDI camera worker is not running"))?;
        handle
            .get_ndi_receiver()
            .await
            .ok_or_else(|| anyhow!("Camera does not have an active NDI receiver"))
    }
}

#[async_trait]
impl PtzDriver for NdiPtzDriver {
    fn protocol_name(&self) -> &'static str {
        "ndi_ptz"
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        let rx = self.active_receiver().await?;
        // NDI convention: +pan is left, -pan is right; invert to match our standard (+pan = right)
        rx.ptz_pan_tilt_speed(-pan, tilt);
        Ok(())
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        let rx = self.active_receiver().await?;
        rx.ptz_zoom_speed(zoom);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let rx = self.active_receiver().await?;
        rx.ptz_pan_tilt_speed(0.0, 0.0);
        rx.ptz_zoom_speed(0.0);
        rx.ptz_focus_speed(0.0);
        Ok(())
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        let rx = self.active_receiver().await?;
        match cmd {
            FocusCommand::Auto | FocusCommand::OnePush => {
                rx.ptz_auto_focus();
            }
            FocusCommand::Manual | FocusCommand::Stop => {
                rx.ptz_focus_speed(0.0);
            }
            FocusCommand::Near { speed } => {
                rx.ptz_focus_speed(-speed.abs());
            }
            FocusCommand::Far { speed } => {
                rx.ptz_focus_speed(speed.abs());
            }
        }
        Ok(())
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        let rx = self.active_receiver().await?;
        rx.ptz_recall_preset(slot as i32, 0.8);
        Ok(())
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        let rx = self.active_receiver().await?;
        rx.ptz_store_preset(slot as i32);
        Ok(())
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        let rx = self.active_receiver().await?;
        match cmd {
            ImagingCommand::WhiteBalanceAuto => {
                rx.ptz_white_balance_auto();
            }
            ImagingCommand::WhiteBalanceIndoor => {
                rx.ptz_white_balance_indoor();
            }
            ImagingCommand::WhiteBalanceOutdoor => {
                rx.ptz_white_balance_outdoor();
            }
            ImagingCommand::WhiteBalanceOnePush => {
                rx.ptz_white_balance_oneshot();
            }
            ImagingCommand::WhiteBalanceManual { red, blue } => {
                rx.ptz_white_balance_manual(red, blue);
            }
            ImagingCommand::ExposureAuto => {
                rx.ptz_exposure_auto();
            }
            ImagingCommand::ExposureManual { level } => {
                rx.ptz_exposure_manual(level);
            }
            ImagingCommand::BacklightComp { .. }
            | ImagingCommand::Sharpness { .. }
            | ImagingCommand::Contrast { .. }
            | ImagingCommand::Brightness { .. } => {}
        }
        Ok(())
    }
}
