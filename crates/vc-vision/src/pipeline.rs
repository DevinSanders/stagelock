//! Per-camera computer vision pipeline unifying:
//! `Detect (ONNX RTMDet or injected detections) -> Pose (RTMPose) -> Appearance Re-ID (OSNet / Spatial HSV)
//!  -> Face-ROI Tenengrad Sharpness -> CMC -> BoT-SORT Identity-Gated Tracking`

use std::path::Path;
use std::time::Instant;

use vc_core::{DetectedSubject, TrackingState};

use crate::cmc::CameraMotionCompensator;
use crate::onnx_engine::{default_models_dir, OnnxVisionEngine, VisionEngineStatus};
use crate::reid::extract_spatial_hsv_descriptor;
use crate::sharpness::compute_subject_face_sharpness;
use crate::tracker::{BotSortTracker, RawDetection};

pub struct VisionFrameOutput {
    pub subjects: Vec<DetectedSubject>,
    pub tracking_state: TrackingState,
    pub locked_track_id: Option<u64>,
    pub locked_face_sharpness: f32,
    pub inference_ms: f32,
}

pub struct CameraVisionPipeline {
    engine: OnnxVisionEngine,
    cmc: CameraMotionCompensator,
    tracker: BotSortTracker,
}

impl CameraVisionPipeline {
    pub fn new_default() -> Self {
        let dir = default_models_dir();
        Self::new_with_models_dir(&dir)
    }

    pub fn new_with_models_dir(models_dir: &Path) -> Self {
        Self {
            engine: OnnxVisionEngine::load_from_dir(models_dir),
            cmc: CameraMotionCompensator::new(),
            tracker: BotSortTracker::new(),
        }
    }

    pub fn status(&self) -> VisionEngineStatus {
        self.engine.status.clone()
    }

    pub fn lock_track(&mut self, track_id: u64) -> Option<u64> {
        self.tracker.lock_track(track_id)
    }

    pub fn lock_at_point(&mut self, norm_x: f32, norm_y: f32) -> Option<u64> {
        self.tracker.lock_at_point(norm_x, norm_y)
    }

    pub fn clear_lock(&mut self) {
        self.tracker.clear_lock();
    }

    /// Process a BGRA video frame. If `seeded_detections` is provided (e.g., synthetic demo stage
    /// when ONNX detector weights aren't installed yet), those boxes are fed through the real
    /// spatial Re-ID, face-ROI Tenengrad sharpness, CMC, and BoT-SORT identity-gating pipeline!
    pub fn process_frame(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        auto_track_enabled: bool,
        seeded_detections: Option<Vec<RawDetection>>,
    ) -> VisionFrameOutput {
        let t0 = Instant::now();

        let mut dets = if self.engine.has_neural_detector() {
            self.engine.detect_persons(bgra, width, height)
        } else {
            seeded_detections.unwrap_or_default()
        };

        // Enrich each detection with Pose (if neural model loaded), Re-ID embedding (OSNet or Spatial HSV),
        // and Face-ROI Tenengrad focus sharpness!
        let mut sharpness_vals = Vec::with_capacity(dets.len());
        let mut fg_boxes = Vec::with_capacity(dets.len());

        for det in &mut dets {
            fg_boxes.push(det.bbox);

            if det.keypoints.is_empty() {
                det.keypoints = self.engine.estimate_pose(bgra, width, height, &det.bbox);
            }

            if det.embedding.is_none() {
                det.embedding = self
                    .engine
                    .extract_reid_embedding(bgra, width, height, &det.bbox)
                    .or_else(|| Some(extract_spatial_hsv_descriptor(bgra, width, height, &det.bbox)));
            }

            let sharp = compute_subject_face_sharpness(bgra, width, height, &det.bbox, &det.keypoints);
            sharpness_vals.push(Some(sharp));
        }

        // Camera Motion Compensation on background tiles (masking out foreground subjects)
        let (cmc_dx, cmc_dy) = self.cmc.estimate_shift(bgra, width, height, &fg_boxes);

        // BoT-SORT association + overlap-frozen operator identity gate
        let snap = self
            .tracker
            .step(dets, &sharpness_vals, cmc_dx, cmc_dy, auto_track_enabled);

        let locked_face_sharpness = snap
            .subjects
            .iter()
            .find(|s| s.is_locked)
            .and_then(|s| s.face_sharpness)
            .or_else(|| snap.subjects.first().and_then(|s| s.face_sharpness))
            .unwrap_or(0.0);

        VisionFrameOutput {
            subjects: snap.subjects,
            tracking_state: snap.tracking_state,
            locked_track_id: snap.locked_track_id,
            locked_face_sharpness,
            inference_ms: t0.elapsed().as_secs_f32() * 1000.0,
        }
    }
}
