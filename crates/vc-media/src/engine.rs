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
use vc_core::{
    CameraConfig, CameraId, CameraStreamStatus, CameraTelemetry, ColorAdjustments,
    FocusMonitorState, FramingConfig, FramingPreset, HardwareAccelBackend, NdiOutputConfig,
    NormalizedRect, PtzVelocity, RuleOfThirds, SystemCapabilities, TallyState, TrackingState,
    VideoSourceConfig,
};
use vc_vision::{CameraVisionPipeline, RawDetection};

use crate::capabilities::probe_system_capabilities;
use crate::color::apply_color_adjustments_bgra;
use crate::gst_pipeline::build_ingest_pipeline_spec;
use crate::gst_runner::{discover_capture_devices, CaptureDeviceInfo, GstFrameSource};
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
    vision_cmd_tx: mpsc::UnboundedSender<VisionCommand>,
    shared: Arc<RwLock<CameraRuntimeShared>>,
}

impl CameraMediaHandle {
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
    task: JoinHandle<()>,
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

        let ndi_runtime = self.ndi_runtime.clone();
        let decode_backend = self.capabilities.decode_backend.clone();
        let worker_shared = shared.clone();
        let task = tokio::task::spawn_blocking(move || {
            run_camera_worker_loop(
                worker_shared,
                ndi_runtime,
                decode_backend,
                preview_tx,
                telemetry_tx,
                vision_cmd_rx,
                stop_rx,
            );
        });

        let handle = CameraMediaHandle {
            camera_id,
            preview_rx,
            telemetry_rx,
            vision_cmd_tx,
            shared,
        };

        self.workers.write().await.insert(
            camera_id,
            ManagedWorker {
                handle: handle.clone(),
                stop_tx,
                task,
            },
        );

        Ok(handle)
    }

    pub async fn stop_camera(&self, id: CameraId) {
        if let Some(worker) = self.workers.write().await.remove(&id) {
            let _ = worker.stop_tx.send(true);
            let _ = tokio::time::timeout(Duration::from_millis(600), worker.task).await;
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
        }
        for w in workers {
            let _ = tokio::time::timeout(Duration::from_millis(800), w.task).await;
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
    preview_tx: watch::Sender<Option<Bytes>>,
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

        if let Some(jpeg) = encode_bgra_to_jpeg(width, height, &frame_buf, 78) {
            let _ = preview_tx.send(Some(jpeg));
        }

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

        tick = tick.wrapping_add(1);
        if !use_gst && ndi_receiver.is_none() {
            let elapsed = loop_start.elapsed();
            if elapsed < frame_period {
                std::thread::sleep(frame_period - elapsed);
            }
        }
    }
}
