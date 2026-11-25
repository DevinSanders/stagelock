use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use bytes::Bytes;
use chrono::Utc;
use jpeg_encoder::{ColorType, Encoder};
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use stagelock_core::{
    CameraConfig, CameraId, CameraStreamStatus, CameraTelemetry, ColorAdjustments,
    FocusMonitorState, FramingConfig, FramingPreset, HardwareAccelBackend, NdiOutputConfig,
    NormalizedRect, PtzVelocity, RuleOfThirds, SystemCapabilities, TallyState, TrackingState,
    VideoSourceConfig,
};
use stagelock_vision::{CameraVisionPipeline, RawDetection};

use crate::capabilities::probe_system_capabilities;
use crate::color::apply_color_adjustments_bgra;
use crate::gst_pipeline::build_ingest_pipeline_spec;
use crate::gst_runner::{discover_capture_devices, CaptureDeviceInfo, GstFrameSource};
use crate::h264_preview::{H264PreviewHub, H264Subscription, RawBgraFrame};
use crate::ndi::{NdiDiscoveredSource, NdiReceiver, NdiRuntime, NdiSender};
use crate::test_pattern::render_demo_frame;

#[derive(Debug, Clone, Copy)]
pub enum VisionCommand {
    LockTrack(u64),
    LockAtPoint(f32, f32),
    ClearLock,
}

pub struct CameraRuntimeShared {
    pub config: CameraConfig,
    pub locked_track_id: Option<u64>,
    pub ptz_velocity: PtzVelocity,
    pub tally: TallyState,
    pub ndi_receiver: Option<Arc<NdiReceiver>>,
    pub target_anchor_override: Option<[f32; 2]>,
    pub deadzone_override: Option<NormalizedRect>,
    pub focus_state_override: Option<FocusMonitorState>,
}

#[derive(Clone)]
pub struct CameraMediaHandle {
    pub camera_id: CameraId,
    pub preview_rx: watch::Receiver<Option<Bytes>>,
    pub telemetry_rx: watch::Receiver<CameraTelemetry>,
    h264_hub: H264PreviewHub,
    vision_cmd_tx: mpsc::UnboundedSender<VisionCommand>,
    shared: Arc<RwLock<CameraRuntimeShared>>,
}

impl CameraMediaHandle {
    pub async fn subscribe_h264(&self) -> Result<H264Subscription> {
        self.h264_hub.subscribe().await
    }
    pub async fn update_color(&self, color: ColorAdjustments) {
        self.shared.write().await.config.color = color;
    }

    pub async fn update_framing(&self, framing: FramingConfig) {
        self.shared.write().await.config.framing = framing;
    }

    pub async fn update_ndi_output(&self, ndi_output: NdiOutputConfig) {
        self.shared.write().await.config.ndi_output = ndi_output;
    }

    pub async fn set_locked_track(&self, track_id: Option<u64>) {
        self.shared.write().await.locked_track_id = track_id;
        match track_id {
            Some(tid) => {
                let _ = self.vision_cmd_tx.send(VisionCommand::LockTrack(tid));
            }
            None => {
                let _ = self.vision_cmd_tx.send(VisionCommand::ClearLock);
            }
        }
    }

    pub async fn set_ptz_velocity(&self, velocity: PtzVelocity) {
        self.shared.write().await.ptz_velocity = velocity;
    }

    pub async fn update_tracking_feedback(
        &self,
        velocity: PtzVelocity,
        target_anchor: [f32; 2],
        deadzone: NormalizedRect,
        focus_state: FocusMonitorState,
    ) {
        let mut guard = self.shared.write().await;
        guard.ptz_velocity = velocity;
        guard.target_anchor_override = Some(target_anchor);
        guard.deadzone_override = Some(deadzone);
        guard.focus_state_override = Some(focus_state);
    }

    pub async fn set_tally(&self, tally: TallyState) {
        self.shared.write().await.tally = tally;
    }

    pub async fn get_ndi_receiver(&self) -> Option<Arc<NdiReceiver>> {
        self.shared.read().await.ndi_receiver.clone()
    }

    pub async fn select_subject_at(&self, norm_x: f32, norm_y: f32) -> Option<u64> {
        let _ = self.vision_cmd_tx.send(VisionCommand::LockAtPoint(norm_x, norm_y));
        let telemetry = self.telemetry_rx.borrow().clone();
        let chosen = telemetry
            .subjects
            .iter()
            .find(|s| {
                norm_x >= s.bbox.x
                    && norm_x <= s.bbox.x + s.bbox.width
                    && norm_y >= s.bbox.y
                    && norm_y <= s.bbox.y + s.bbox.height
            })
            .map(|s| s.track_id);
        self.shared.write().await.locked_track_id = chosen;
        chosen
    }
}

struct ManagedWorker {
    handle: CameraMediaHandle,
    stop_tx: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    jpeg_task: JoinHandle<()>,
}

impl Drop for ManagedWorker {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        self.jpeg_task.abort();
    }
}

#[derive(Clone)]
pub struct MediaEngine {
    capabilities: SystemCapabilities,
    ndi_runtime: Option<NdiRuntime>,
    workers: Arc<RwLock<HashMap<CameraId, ManagedWorker>>>,
}

impl MediaEngine {
    pub fn new() -> Self {
        let capabilities = probe_system_capabilities();
        let ndi_runtime = match NdiRuntime::try_load() {
            Ok(rt) => Some(rt),
            Err(err) => {
                warn!("NDI runtime unavailable: {err}");
                None
            }
        };

        info!(
            decode = ?capabilities.decode_backend,
            encode = ?capabilities.encode_backend,
            ndi = capabilities.ndi_runtime_available,
            gst = capabilities.gstreamer_available,
            "Initialized MediaEngine"
        );

        Self {
            capabilities,
            ndi_runtime,
            workers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn capabilities(&self) -> SystemCapabilities {
        self.capabilities.clone()
    }

    pub fn ndi_runtime(&self) -> Option<NdiRuntime> {
        self.ndi_runtime.clone()
    }

    pub async fn discover_ndi_sources(&self) -> Vec<NdiDiscoveredSource> {
        let Some(ndi) = self.ndi_runtime.clone() else {
            return Vec::new();
        };
        tokio::task::spawn_blocking(move || ndi.discover_sources(1200).unwrap_or_default())
            .await
            .unwrap_or_default()
    }

    pub async fn discover_usb_devices(&self) -> Vec<CaptureDeviceInfo> {
        tokio::task::spawn_blocking(discover_capture_devices)
            .await
            .unwrap_or_default()
    }

    pub async fn start_or_restart_camera(&self, config: CameraConfig) -> Result<CameraMediaHandle> {
        self.stop_camera(config.id).await;

        let camera_id = config.id;
        let pipeline_spec = build_ingest_pipeline_spec(
            &config.source,
            &self.capabilities.decode_backend,
            &self.capabilities.encode_backend,
            config
                .ndi_output
                .enabled
                .then_some(config.ndi_output.stream_name.as_str()),
        );
        info!(camera_id = %camera_id, name = %config.name, pipeline = %pipeline_spec, "Starting camera worker");

        let (preview_tx, preview_rx) = watch::channel(None);
        let (raw_frame_tx, raw_frame_rx) = watch::channel::<Option<RawBgraFrame>>(None);
        let initial_telemetry = make_initial_telemetry(&config);
        let (telemetry_tx, telemetry_rx) = watch::channel(initial_telemetry);
        let (stop_tx, stop_rx) = watch::channel(false);
        let (vision_cmd_tx, vision_cmd_rx) = mpsc::unbounded_channel();

        let shared = Arc::new(RwLock::new(CameraRuntimeShared {
            config: config.clone(),
            locked_track_id: None,
            ptz_velocity: PtzVelocity::default(),
            tally: TallyState::Off,
            ndi_receiver: None,
            target_anchor_override: None,
            deadzone_override: None,
            focus_state_override: None,
        }));

        let h264_hub = H264PreviewHub::new(
            self.capabilities.encode_backend.clone(),
            raw_frame_rx.clone(),
            stop_rx.clone(),
        );

        // Decoupled async JPEG preview encoder worker: reads always-latest BGRA frame without
        // ever stalling the capture, AI vision, or PTZ control loop.
        let mut jpeg_raw_rx = raw_frame_rx.clone();
        let mut jpeg_stop_rx = stop_rx.clone();
        let jpeg_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = jpeg_stop_rx.changed() => {
                        if *jpeg_stop_rx.borrow() {
                            break;
                        }
                    }
                    res = jpeg_raw_rx.changed() => {
                        if res.is_err() {
                            break;
                        }
                        let Some(frame) = jpeg_raw_rx.borrow().clone() else {
                            continue;
                        };
                        let encoded = tokio::task::spawn_blocking(move || {
                            encode_bgra_to_jpeg(frame.width, frame.height, &frame.bgra, 78)
                        })
                        .await
                        .ok()
                        .flatten();
                        if let Some(jpeg) = encoded {
                            let _ = preview_tx.send(Some(jpeg));
                        }
                    }
                }
            }
        });

        let ndi_runtime = self.ndi_runtime.clone();
        let decode_backend = self.capabilities.decode_backend.clone();
        let worker_shared = shared.clone();
        let task = tokio::task::spawn_blocking(move || {
            run_camera_worker_loop(
                worker_shared,
                ndi_runtime,
                decode_backend,
                raw_frame_tx,
                telemetry_tx,
                vision_cmd_rx,
                stop_rx,
            );
        });

        let handle = CameraMediaHandle {
            camera_id,
            preview_rx,
            telemetry_rx,
            h264_hub,
            vision_cmd_tx,
            shared,
        };

        self.workers.write().await.insert(
            camera_id,
            ManagedWorker {
                handle: handle.clone(),
                stop_tx,
                task: Some(task),
                jpeg_task,
            },
        );

        Ok(handle)
    }

    pub async fn stop_camera(&self, id: CameraId) {
        if let Some(mut worker) = self.workers.write().await.remove(&id) {
            let _ = worker.stop_tx.send(true);
            worker.jpeg_task.abort();
            if let Some(task) = worker.task.take() {
                let _ = tokio::time::timeout(Duration::from_millis(600), task).await;
            }
        }
    }

    /// Gracefully stop all active camera capture/NDI/GStreamer workers on daemon shutdown.
    pub async fn stop_all_cameras(&self) {
        let workers: Vec<ManagedWorker> = self
            .workers
            .write()
            .await
            .drain()
            .map(|(_, w)| w)
            .collect();
        for w in &workers {
            let _ = w.stop_tx.send(true);
            w.jpeg_task.abort();
        }
        for mut w in workers {
            if let Some(task) = w.task.take() {
                let _ = tokio::time::timeout(Duration::from_millis(800), task).await;
            }
        }
    }

    pub async fn get_handle(&self, id: CameraId) -> Option<CameraMediaHandle> {
        self.workers.read().await.get(&id).map(|w| w.handle.clone())
    }

    pub async fn collect_telemetry(&self) -> Vec<CameraTelemetry> {
        let guard = self.workers.read().await;
        guard
            .values()
            .map(|w| w.handle.telemetry_rx.borrow().clone())
            .collect()
    }
}

fn make_initial_telemetry(config: &CameraConfig) -> CameraTelemetry {
    let (anchor_x, anchor_y) = compute_target_anchor(config.framing.preset, config.framing.rule_of_thirds);
    CameraTelemetry {
        camera_id: config.id,
        timestamp_ms: Utc::now().timestamp_millis(),
        stream_status: CameraStreamStatus::Connecting,
        ndi_output_active: false,
        tally: TallyState::Off,
        tracking_state: TrackingState::Idle,
        auto_track_enabled: config.framing.auto_track_enabled,
        locked_track_id: None,
        framing_preset: config.framing.preset,
        rule_of_thirds: config.framing.rule_of_thirds,
        target_anchor: [anchor_x, anchor_y],
        deadzone: NormalizedRect {
            x: (anchor_x - config.framing.deadzone_x).clamp(0.0, 1.0),
            y: (anchor_y - config.framing.deadzone_y).clamp(0.0, 1.0),
            width: config.framing.deadzone_x * 2.0,
            height: config.framing.deadzone_y * 2.0,
        },
        focus_state: FocusMonitorState::Sharp,
        focus_sharpness: 0.85,
        ptz_velocity: PtzVelocity::default(),
        subjects: Vec::new(),
        preview_fps: 0.0,
        inference_fps: 0.0,
    }
}

pub fn compute_target_anchor(preset: FramingPreset, thirds: RuleOfThirds) -> (f32, f32) {
    let x = match thirds {
        RuleOfThirds::Center => 0.50,
        RuleOfThirds::LeftThird => 0.333,
        RuleOfThirds::RightThird => 0.667,
        RuleOfThirds::AutoLeadRoom => 0.38,
    };
    let y = match preset {
        FramingPreset::CloseUp => 0.333,
        FramingPreset::WaistUp => 0.29,
        FramingPreset::FullHeight => 0.17,
    };
    (x, y)
}

fn encode_bgra_to_jpeg(width: u32, height: u32, bgra: &[u8], quality: u8) -> Option<Bytes> {
    let mut out = Vec::with_capacity((width * height / 6) as usize);
    let encoder = Encoder::new(&mut out, quality);
    encoder
        .encode(bgra, width as u16, height as u16, ColorType::Bgra)
        .ok()?;
    Some(Bytes::from(out))
}

fn run_camera_worker_loop(
    shared: Arc<RwLock<CameraRuntimeShared>>,
    ndi_runtime: Option<NdiRuntime>,
    decode_backend: HardwareAccelBackend,
    raw_frame_tx: watch::Sender<Option<RawBgraFrame>>,
    telemetry_tx: watch::Sender<CameraTelemetry>,
    mut vision_cmd_rx: mpsc::UnboundedReceiver<VisionCommand>,
    stop_rx: watch::Receiver<bool>,
) {
    let initial = shared.blocking_read().config.clone();
    let mut vision = CameraVisionPipeline::new_default();

    let (mut width, mut height, fps, pattern, ndi_src, use_gst) = match &initial.source {
        VideoSourceConfig::TestPattern {
            pattern,
            width,
            height,
            fps,
        } => (
            (*width).clamp(320, 1920),
            (*height).clamp(180, 1080),
            (*fps).clamp(5, 60),
            pattern.clone(),
            None,
            false,
        ),
        VideoSourceConfig::Ndi {
            source_name,
            low_bandwidth,
        } => (
            1280,
            720,
            30,
            "ndi".to_string(),
            Some((source_name.clone(), *low_bandwidth)),
            false,
        ),
        VideoSourceConfig::Usb { width, height, fps, .. } => (
            (*width).clamp(320, 1920),
            (*height).clamp(180, 1080),
            (*fps).clamp(5, 60),
            "usb".to_string(),
            None,
            true,
        ),
        _ => (1280, 720, 30, "gst".to_string(), None, true),
    };

    let ndi_receiver: Option<Arc<NdiReceiver>> = ndi_src.and_then(|(name, low_bw)| {
        ndi_runtime
            .as_ref()
            .and_then(|rt| match rt.create_receiver(&name, low_bw) {
                Ok(rx) => {
                    let arc_rx = Arc::new(rx);
                    shared.blocking_write().ndi_receiver = Some(arc_rx.clone());
                    Some(arc_rx)
                }
                Err(e) => {
                    warn!(source = %name, "Failed opening NDI receiver: {e}");
                    None
                }
            })
    });

    let mut gst_source: Option<GstFrameSource> = if use_gst {
        match GstFrameSource::start(&initial.source, &decode_backend, width, height, fps) {
            Ok(src) => Some(src),
            Err(err) => {
                warn!(camera = %initial.name, "GStreamer ingest start failed (will retry): {err}");
                None
            }
        }
    } else {
        None
    };

    let mut reconnect_attempt: u32 = 0;
    let mut last_reconnect_try = Instant::now() - Duration::from_secs(10);
    let mut active_ndi_sender: Option<(String, NdiSender)> = None;
    let mut frame_buf = vec![0u8; (width * height * 4) as usize];
    let frame_period = Duration::from_millis((1000 / fps.max(1)) as u64);
    let mut tick: u64 = 0;

    while !*stop_rx.borrow() {
        let loop_start = Instant::now();

        // Drain operator subject-selection / clear-lock commands
        while let Ok(cmd) = vision_cmd_rx.try_recv() {
            match cmd {
                VisionCommand::LockTrack(tid) => {
                    vision.lock_track(tid);
                }
                VisionCommand::LockAtPoint(x, y) => {
                    vision.lock_at_point(x, y);
                }
                VisionCommand::ClearLock => {
                    vision.clear_lock();
                }
            }
        }

        let state_snapshot = {
            let guard = shared.blocking_read();
            (
                guard.config.clone(),
                guard.ptz_velocity,
                guard.tally,
                guard.target_anchor_override,
                guard.deadzone_override,
                guard.focus_state_override,
            )
        };
        let (cfg, ptz_velocity, tally, anchor_override, dz_override, focus_override) = state_snapshot;

        // Reconcile NDI re-broadcast sender dynamically
        if cfg.ndi_output.enabled {
            let needs_create = match &active_ndi_sender {
                Some((existing_name, _)) => existing_name != &cfg.ndi_output.stream_name,
                None => true,
            };
            if needs_create {
                active_ndi_sender = ndi_runtime.as_ref().and_then(|rt| {
                    match rt.create_sender(
                        &cfg.ndi_output.stream_name,
                        cfg.ndi_output.groups.as_deref(),
                    ) {
                        Ok(sender) => Some((cfg.ndi_output.stream_name.clone(), sender)),
                        Err(e) => {
                            warn!("Failed starting NDI sender: {e}");
                            None
                        }
                    }
                });
            }
        } else if active_ndi_sender.is_some() {
            active_ndi_sender = None;
        }

        // Acquire next frame from live NDI, live GPU GStreamer pipeline, or synthetic stage
        let mut stream_status_override: Option<CameraStreamStatus> = None;
        let (seeded_dets, decoder_name, hw_accelerated) = if let Some(ref rx) = ndi_receiver {
            if let Some(ndi_frame) = rx.capture_bgra(100) {
                width = ndi_frame.width;
                height = ndi_frame.height;
                frame_buf = ndi_frame.bgra;
                (None, "NDI 6 Runtime".to_string(), true)
            } else {
                continue;
            }
        } else if use_gst {
            if gst_source.is_none() {
                let backoff = Duration::from_secs((2_u64.pow(reconnect_attempt.min(4))).min(15));
                if last_reconnect_try.elapsed() >= backoff {
                    last_reconnect_try = Instant::now();
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                    match GstFrameSource::start(&cfg.source, &decode_backend, width, height, fps) {
                        Ok(src) => {
                            reconnect_attempt = 0;
                            gst_source = Some(src);
                        }
                        Err(err) => {
                            stream_status_override = Some(CameraStreamStatus::Reconnecting {
                                attempt: reconnect_attempt,
                                last_error: err.to_string(),
                            });
                        }
                    }
                }
            }

            if let Some(ref mut src) = gst_source {
                if let Some(captured) = src.next_frame() {
                    width = captured.width;
                    height = captured.height;
                    frame_buf = captured.bgra;
                    (None, src.decoder_label.clone(), src.hw_accelerated)
                } else {
                    warn!(camera = %cfg.name, "Live GStreamer feed disconnected; scheduling reconnect");
                    gst_source = None;
                    stream_status_override = Some(CameraStreamStatus::Reconnecting {
                        attempt: reconnect_attempt.saturating_add(1),
                        last_error: "Stream ended or timed out".to_string(),
                    });
                    (None, "Reconnecting...".to_string(), false)
                }
            } else {
                let (anchor_x, anchor_y) = compute_target_anchor(cfg.framing.preset, cfg.framing.rule_of_thirds);
                let _ = telemetry_tx.send(CameraTelemetry {
                    camera_id: cfg.id,
                    timestamp_ms: Utc::now().timestamp_millis(),
                    stream_status: stream_status_override.unwrap_or(CameraStreamStatus::Connecting),
                    ndi_output_active: active_ndi_sender.is_some(),
                    tally,
                    tracking_state: TrackingState::Idle,
                    auto_track_enabled: cfg.framing.auto_track_enabled,
                    locked_track_id: None,
                    framing_preset: cfg.framing.preset,
                    rule_of_thirds: cfg.framing.rule_of_thirds,
                    target_anchor: [anchor_x, anchor_y],
                    deadzone: NormalizedRect {
                        x: (anchor_x - cfg.framing.deadzone_x).clamp(0.0, 1.0),
                        y: (anchor_y - cfg.framing.deadzone_y).clamp(0.0, 1.0),
                        width: cfg.framing.deadzone_x * 2.0,
                        height: cfg.framing.deadzone_y * 2.0,
                    },
                    focus_state: FocusMonitorState::Sharp,
                    focus_sharpness: 0.0,
                    ptz_velocity,
                    subjects: Vec::new(),
                    preview_fps: 0.0,
                    inference_fps: 0.0,
                });
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        } else {
            let required = (width * height * 4) as usize;
            if frame_buf.len() != required {
                frame_buf.resize(required, 0);
            }
            let demo_subs = render_demo_frame(width, height, &pattern, tick, None, &mut frame_buf);
            let raw: Vec<RawDetection> = demo_subs
                .into_iter()
                .map(|s| RawDetection {
                    bbox: s.bbox,
                    confidence: s.confidence,
                    keypoints: s.keypoints,
                    embedding: None,
                })
                .collect();
            (Some(raw), "Synthetic Studio Feed".to_string(), false)
        };

        // Run per-camera CV pipeline (ONNX RTMDet/RTMPose/OSNet or Spatial HSV Re-ID + Face-ROI Sharpness + CMC + BoT-SORT)
        let vision_out = vision.process_frame(
            &frame_buf,
            width,
            height,
            cfg.framing.auto_track_enabled,
            seeded_dets,
        );

        // Publish telemetry IMMEDIATELY after AI inference (BEFORE color grading & preview encoding!)
        // so the event-driven PTZ tracking loop reacts within <0.2ms of each detection.
        let (default_ax, default_ay) = compute_target_anchor(cfg.framing.preset, cfg.framing.rule_of_thirds);
        let target_anchor = anchor_override.unwrap_or([default_ax, default_ay]);
        let deadzone = dz_override.unwrap_or(NormalizedRect {
            x: (target_anchor[0] - cfg.framing.deadzone_x).clamp(0.0, 1.0),
            y: (target_anchor[1] - cfg.framing.deadzone_y).clamp(0.0, 1.0),
            width: cfg.framing.deadzone_x * 2.0,
            height: cfg.framing.deadzone_y * 2.0,
        });
        let focus_state = focus_override.unwrap_or_else(|| {
            if vision_out.locked_face_sharpness > 0.0 && vision_out.locked_face_sharpness < 0.25 {
                FocusMonitorState::Defocused
            } else {
                FocusMonitorState::Sharp
            }
        });
        let inference_fps = if vision_out.inference_ms > 0.05 {
            (1000.0 / vision_out.inference_ms).min(fps as f32)
        } else {
            fps as f32
        };

        let telemetry = CameraTelemetry {
            camera_id: cfg.id,
            timestamp_ms: Utc::now().timestamp_millis(),
            stream_status: stream_status_override.unwrap_or(CameraStreamStatus::Running {
                width,
                height,
                fps: fps as f32,
                hw_accelerated,
                decoder_name,
            }),
            ndi_output_active: active_ndi_sender.is_some(),
            tally,
            tracking_state: vision_out.tracking_state,
            auto_track_enabled: cfg.framing.auto_track_enabled,
            locked_track_id: vision_out.locked_track_id,
            framing_preset: cfg.framing.preset,
            rule_of_thirds: cfg.framing.rule_of_thirds,
            target_anchor,
            deadzone,
            focus_state,
            focus_sharpness: vision_out.locked_face_sharpness,
            ptz_velocity,
            subjects: vision_out.subjects,
            preview_fps: fps as f32,
            inference_fps,
        };
        let _ = telemetry_tx.send(telemetry);

        // Optional clean NDI re-broadcast before color adjustments
        if !cfg.ndi_output.include_color_adjustments {
            if let Some((_, ref sender)) = active_ndi_sender {
                sender.send_bgra_frame(width, height, fps, &frame_buf);
            }
        }

        // Apply live color adjustments + Unsharp-Mask Sharpness
        apply_color_adjustments_bgra(&mut frame_buf, width, height, &cfg.color);

        // NDI re-broadcast with color adjustments baked in
        if cfg.ndi_output.include_color_adjustments {
            if let Some((_, ref sender)) = active_ndi_sender {
                sender.send_bgra_frame(width, height, fps, &frame_buf);
            }
        }

        // Hand off latest graded BGRA frame non-blockingly to decoupled JPEG & GPU H.264 preview encoders
        let timestamp_us = (tick.saturating_mul(1_000_000)) / (fps.max(1) as u64);
        let _ = raw_frame_tx.send(Some(RawBgraFrame {
            width,
            height,
            fps,
            timestamp_us,
            bgra: Arc::new(frame_buf.clone()),
        }));

        tick = tick.wrapping_add(1);
        if !use_gst && ndi_receiver.is_none() {
            let elapsed = loop_start.elapsed();
            if elapsed < frame_period {
                std::thread::sleep(frame_period - elapsed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_target_anchor_covers_all_presets_and_thirds() {
        for preset in [FramingPreset::CloseUp, FramingPreset::WaistUp, FramingPreset::FullHeight] {
            for thirds in [
                RuleOfThirds::Center,
                RuleOfThirds::LeftThird,
                RuleOfThirds::RightThird,
                RuleOfThirds::AutoLeadRoom,
            ] {
                let (x, y) = compute_target_anchor(preset, thirds);
                assert!((0.1..=0.9).contains(&x) && (0.1..=0.9).contains(&y));
            }
        }
    }

    #[tokio::test]
    async fn media_engine_runs_demo_camera_updates_controls_and_stops_cleanly() {
        let engine = MediaEngine::new();
        assert!(!engine.capabilities().os.is_empty());
        let _ = engine.ndi_runtime();
        let _ = engine.discover_usb_devices().await;
        let _ = engine.discover_ndi_sources().await;

        let mut cam = CameraConfig::new_demo("Engine Test Cam", "smpte");
        cam.source = VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 320,
            height: 180,
            fps: 30,
        };
        cam.ndi_output.enabled = true;
        cam.ndi_output.include_color_adjustments = true;
        cam.ndi_output.stream_name = "StageLock-EngineTest".into();
        let cam_id = cam.id;

        let handle = engine.start_or_restart_camera(cam).await.unwrap();
        assert!(engine.get_handle(cam_id).await.is_some());

        // Wait for live JPEG preview & telemetry with subjects
        let preview_rx = handle.preview_rx.clone();
        let telemetry_rx = handle.telemetry_rx.clone();
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let has_jpg = preview_rx.borrow().as_ref().map_or(false, |b| b.len() > 100);
                let has_subjs = !telemetry_rx.borrow().subjects.is_empty();
                if has_jpg && has_subjs {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("demo camera should emit JPEG and telemetry");

        // Exercise every CameraMediaHandle mutation
        handle
            .update_color(ColorAdjustments {
                brightness: 0.1,
                contrast: 1.1,
                ..ColorAdjustments::default()
            })
            .await;
        handle
            .update_framing(FramingConfig {
                auto_track_enabled: true,
                preset: FramingPreset::CloseUp,
                rule_of_thirds: RuleOfThirds::LeftThird,
                ..FramingConfig::default()
            })
            .await;
        handle
            .update_ndi_output(NdiOutputConfig {
                enabled: true,
                stream_name: "StageLock-EngineTest-Clean".into(),
                groups: None,
                include_color_adjustments: false,
            })
            .await;
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Disable NDI output to exercise sender teardown branch
        handle
            .update_ndi_output(NdiOutputConfig {
                enabled: false,
                stream_name: "StageLock-EngineTest-Clean".into(),
                groups: None,
                include_color_adjustments: false,
            })
            .await;

        let subj = telemetry_rx.borrow().subjects[0].clone();
        let picked = handle
            .select_subject_at(
                subj.bbox.x + subj.bbox.width * 0.5,
                subj.bbox.y + subj.bbox.height * 0.5,
            )
            .await;
        assert_eq!(picked, Some(subj.track_id));
        handle.set_locked_track(Some(subj.track_id)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.set_locked_track(None).await;
        handle
            .set_ptz_velocity(PtzVelocity {
                pan: 0.2,
                tilt: -0.1,
                zoom: 0.05,
            })
            .await;
        handle
            .update_tracking_feedback(
                PtzVelocity { pan: 0.0, tilt: 0.0, zoom: 0.0 },
                [0.33, 0.33],
                NormalizedRect { x: 0.25, y: 0.25, width: 0.16, height: 0.16 },
                FocusMonitorState::Sharp,
            )
            .await;
        handle.set_tally(TallyState::Program).await;
        assert!(handle.get_ndi_receiver().await.is_none());

        // Subscribe H.264 if host has an encoder (drops cleanly)
        if let Ok(_sub) = handle.subscribe_h264().await {}

        // Also test unreachable RTSP failure/reconnect branch briefly
        let mut bad_cam = CameraConfig::new_demo("Unreachable RTSP", "smpte");
        bad_cam.source = VideoSourceConfig::Rtsp {
            url: "rtsp://127.0.0.1:1/nonexistent".into(),
            latency_ms: 25,
            prefer_tcp: true,
        };
        let _ = engine.start_or_restart_camera(bad_cam).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;

        let all = engine.collect_telemetry().await;
        assert_eq!(all.len(), 2);

        engine.stop_all_cameras().await;
        assert!(engine.collect_telemetry().await.is_empty());
    }
}
