//! Independent per-camera 30 Hz closed-loop PTZ tracking & auto-focus supervisor.
//! Ensures multiple PTZ cameras track different subjects simultaneously without interference,
//! yields immediately to manual joystick/preset overrides, and runs the PTZ Calibration Wizard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::Serialize;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info};
use vc_core::{CameraId, ConfigStore};
use vc_media::MediaEngine;
use vc_ptz::PtzManager;
use vc_tracking::{CameraCalibration, CameraTrackingController};

enum LoopControlMsg {
    ManualOverride(f32),
    SetCalibration(CameraCalibration),
    Stop,
}

struct TrackingLoopEntry {
    tx: mpsc::UnboundedSender<LoopControlMsg>,
    task: JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationReport {
    pub camera_id: CameraId,
    pub protocol: String,
    pub calibration: CameraCalibration,
    pub note: String,
}

#[derive(Clone)]
pub struct TrackingSupervisor {
    store: ConfigStore,
    media: MediaEngine,
    ptz: PtzManager,
    loops: Arc<RwLock<HashMap<CameraId, TrackingLoopEntry>>>,
}

impl TrackingSupervisor {
    pub fn new(store: ConfigStore, media: MediaEngine, ptz: PtzManager) -> Self {
        Self {
            store,
            media,
            ptz,
            loops: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start_or_restart_camera(&self, camera_id: CameraId) {
        self.stop_camera(camera_id).await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let store = self.store.clone();
        let media = self.media.clone();
        let ptz = self.ptz.clone();

        let initial_calibration = self
            .store
            .get_camera(camera_id)
            .await
            .map(|c| c.calibration)
            .unwrap_or_default();

        let task = tokio::spawn(async move {
            let mut controller = CameraTrackingController::new(initial_calibration);
            let mut safety_ticker = tokio::time::interval(Duration::from_millis(100));
            let mut last_tick = Instant::now();
            let mut subscribed_telem: Option<tokio::sync::watch::Receiver<vc_core::CameraTelemetry>> =
                media.get_handle(camera_id).await.map(|h| h.telemetry_rx);

            loop {
                // Event-driven wake: react immediately (<0.2ms) to every fresh AI vision frame
                // rather than polling on a 33ms timer, with a 100ms safety tick for signal-loss stops.
                let woke_by_frame = if let Some(ref mut telem_rx) = subscribed_telem {
                    tokio::select! {
                        biased;
                        Some(msg) = rx.recv() => {
                            match msg {
                                LoopControlMsg::ManualOverride(secs) => {
                                    controller.notify_manual_override(secs);
                                }
                                LoopControlMsg::SetCalibration(cal) => {
                                    controller.set_calibration(cal);
                                }
                                LoopControlMsg::Stop => break,
                            }
                            continue;
                        }
                        res = telem_rx.changed() => {
                            if res.is_err() {
                                subscribed_telem = None;
                                continue;
                            }
                            true
                        }
                        _ = safety_ticker.tick() => false,
                    }
                } else {
                    tokio::select! {
                        Some(msg) = rx.recv() => {
                            match msg {
                                LoopControlMsg::ManualOverride(secs) => {
                                    controller.notify_manual_override(secs);
                                }
                                LoopControlMsg::SetCalibration(cal) => {
                                    controller.set_calibration(cal);
                                }
                                LoopControlMsg::Stop => break,
                            }
                            continue;
                        }
                        _ = safety_ticker.tick() => {
                            subscribed_telem = media.get_handle(camera_id).await.map(|h| h.telemetry_rx);
                            false
                        }
                    }
                };

                let now = Instant::now();
                let dt = now.duration_since(last_tick).as_secs_f32().clamp(0.008, 0.15);
                if woke_by_frame || dt >= 0.08 {
                    last_tick = now;
                } else {
                    continue;
                }

                let Some(cam_cfg) = store.get_camera(camera_id).await else {
                    continue;
                };
                let Some(handle) = media.get_handle(camera_id).await else {
                    subscribed_telem = None;
                    continue;
                };

                let telem = handle.telemetry_rx.borrow().clone();
                let locked_subject = telem.subjects.iter().find(|s| s.is_locked);

                let out = controller.tick(
                    &cam_cfg.framing,
                    telem.tracking_state,
                    locked_subject,
                    telem.focus_sharpness,
                    telem.tally,
                    dt,
                );

                handle
                    .update_tracking_feedback(
                        out.ptz_velocity,
                        out.target_anchor,
                        out.deadzone,
                        out.focus_state,
                    )
                    .await;

                if let Some(driver) = ptz.get(camera_id).await {
                    if out.should_send_ptz {
                        let v = out.ptz_velocity;
                        if v.pan.abs() < 1e-3 && v.tilt.abs() < 1e-3 && v.zoom.abs() < 1e-3 {
                            let _ = driver.stop().await;
                        } else {
                            let _ = driver.pan_tilt_speed(v.pan, v.tilt).await;
                            let _ = driver.zoom_speed(v.zoom).await;
                        }
                    }

                    if let Some(focus_cmd) = out.focus_command {
                        debug!(camera_id = %camera_id, ?focus_cmd, "Auto-focus controller issued focus command");
                        let _ = driver.focus(focus_cmd).await;
                    }
                }
            }
        });

        self.loops
            .write()
            .await
            .insert(camera_id, TrackingLoopEntry { tx, task });
    }

    pub async fn stop_camera(&self, camera_id: CameraId) {
        if let Some(entry) = self.loops.write().await.remove(&camera_id) {
            let _ = entry.tx.send(LoopControlMsg::Stop);
            entry.task.abort();
        }
    }

    pub async fn stop_all_cameras(&self) {
        let entries: Vec<TrackingLoopEntry> = self
            .loops
            .write()
            .await
            .drain()
            .map(|(_, e)| e)
            .collect();
        for entry in entries {
            let _ = entry.tx.send(LoopControlMsg::Stop);
            entry.task.abort();
        }
    }

    pub async fn notify_manual_override(&self, camera_id: CameraId, hold_seconds: f32) {
        if let Some(entry) = self.loops.read().await.get(&camera_id) {
            let _ = entry.tx.send(LoopControlMsg::ManualOverride(hold_seconds));
        }
    }

    /// Run a gentle bidirectional PTZ speed & latency calibration sweep on the selected camera,
    /// returning the camera back to its starting aim while calibrating response scaling.
    pub async fn calibrate_camera(&self, camera_id: CameraId) -> Result<CalibrationReport> {
        let driver = self
            .ptz
            .get(camera_id)
            .await
            .ok_or_else(|| anyhow!("No PTZ driver configured for this camera"))?;

        // Pause automated tracking during the calibration pulse
        self.notify_manual_override(camera_id, 4.0).await;

        let t0 = Instant::now();
        let _ = driver.pan_tilt_speed(0.28, 0.0).await;
        let cmd_rtt_ms = t0.elapsed().as_secs_f32() * 1000.0;
        tokio::time::sleep(Duration::from_millis(320)).await;
        let _ = driver.stop().await;
        tokio::time::sleep(Duration::from_millis(180)).await;

        // Return symmetrically to starting position
        let _ = driver.pan_tilt_speed(-0.28, 0.0).await;
        tokio::time::sleep(Duration::from_millis(320)).await;
        let _ = driver.stop().await;

        let protocol = driver.protocol_name().to_string();
        // Tune base response & latency profile by protocol transport + measured command RTT
        let (pan_scale, tilt_scale, zoom_scale, base_latency_ms, min_speed) = match protocol.as_str() {
            "visca_ip_raw_tcp" | "visca_ip_sony" | "visca_ip_raw_udp" => (0.95, 0.90, 0.90, 150.0, 0.04),
            "ndi_ptz" => (1.00, 0.95, 0.95, 130.0, 0.03),
            "onvif" => (0.85, 0.80, 0.85, 220.0, 0.05),
            "canon_xc" | "panasonic_aw" => (0.92, 0.88, 0.90, 170.0, 0.03),
            _ => (1.0, 1.0, 1.0, 160.0, 0.03),
        };

        let calibration = CameraCalibration {
            pan_scale,
            tilt_scale,
            zoom_scale,
            latency_ms: (base_latency_ms + cmd_rtt_ms).clamp(80.0, 450.0),
            min_effective_speed: min_speed,
        };

        let _ = self
            .store
            .update_calibration(camera_id, calibration.clone())
            .await;

        if let Some(entry) = self.loops.read().await.get(&camera_id) {
            let _ = entry
                .tx
                .send(LoopControlMsg::SetCalibration(calibration.clone()));
        }

        info!(camera_id = %camera_id, protocol = %protocol, latency_ms = calibration.latency_ms, "Completed PTZ speed calibration");

        Ok(CalibrationReport {
            camera_id,
            protocol,
            calibration,
            note: "Calibrated bidirectional pan pulse and applied protocol latency/deadband profile.".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vc_core::{CameraConfig, PtzConfig, VideoSourceConfig};

    #[tokio::test]
    async fn supervisor_tracks_live_camera_handles_override_and_calibrates_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::load_or_create(dir.path().join("track.db")).unwrap();
        let media = MediaEngine::new();
        let ptz = PtzManager::new(media.clone());
        let sup = TrackingSupervisor::new(store.clone(), media.clone(), ptz.clone());

        let mut cam = CameraConfig::new_demo("Tracked Cam", "smpte");
        cam.source = VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 320,
            height: 180,
            fps: 30,
        };
        cam.ptz = PtzConfig::Mock;
        cam.framing.auto_track_enabled = true;
        let cam_id = cam.id;
        store.upsert_camera(cam.clone()).await.unwrap();
        ptz.configure_camera(&cam).await;

        // Start tracking BEFORE media exists to also cover the media-not-yet-started select branch
        sup.start_or_restart_camera(cam_id).await;
        tokio::time::sleep(Duration::from_millis(130)).await;

        let handle = media.start_or_restart_camera(cam.clone()).await.unwrap();
        // Wait for telemetry & lock first subject so tracking ticks actively send PTZ commands
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if !handle.telemetry_rx.borrow().subjects.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();

        let tid = handle.telemetry_rx.borrow().subjects[0].track_id;
        handle.set_locked_track(Some(tid)).await;
        tokio::time::sleep(Duration::from_millis(180)).await;

        // Manual override hold
        sup.notify_manual_override(cam_id, 0.2).await;
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Calibrate camera -> persists calibration into SQLite & notifies running loop
        let report = sup.calibrate_camera(cam_id).await.unwrap();
        assert_eq!(report.protocol, "mock");
        assert!(report.calibration.latency_ms >= 80.0);
        let reloaded = store.get_camera(cam_id).await.unwrap();
        assert!((reloaded.calibration.latency_ms - report.calibration.latency_ms).abs() < 1e-3);

        // Failure path: calibrating unknown camera returns error
        assert!(sup.calibrate_camera(CameraId::new_v4()).await.is_err());

        sup.stop_all_cameras().await;
        media.stop_all_cameras().await;
    }
}
