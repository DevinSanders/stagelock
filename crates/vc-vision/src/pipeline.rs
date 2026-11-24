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

#[cfg(test)]
mod tests {
    use super::*;
    use vc_core::NormalizedRect;

    #[test]
    fn vision_pipeline_end_to_end_lock_point_and_clear() {
        let mut pipe = CameraVisionPipeline::new_default();
        assert_eq!(pipe.status().models.len(), 3);

        let w = 128_u32;
        let h = 96_u32;
        let mut frame = vec![40_u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                frame[i] = ((x * 2) % 255) as u8;
                frame[i + 1] = ((y * 3) % 255) as u8;
                frame[i + 2] = 180;
                frame[i + 3] = 255;
            }
        }

        let det = RawDetection {
            bbox: NormalizedRect { x: 0.25, y: 0.15, width: 0.30, height: 0.70 },
            confidence: 0.94,
            keypoints: Vec::new(),
            embedding: None,
        };

        let out1 = pipe.process_frame(&frame, w, h, true, Some(vec![det.clone()]));
        assert_eq!(out1.subjects.len(), 1);
        assert_eq!(out1.tracking_state, TrackingState::Searching);

        // Lock at point inside subject
        let tid = pipe.lock_at_point(0.40, 0.45).expect("should lock subject at click");
        assert_eq!(pipe.lock_track(tid), Some(tid));

        let out2 = pipe.process_frame(&frame, w, h, true, Some(vec![det]));
        assert_eq!(out2.tracking_state, TrackingState::Locked);
        assert_eq!(out2.locked_track_id, Some(tid));
        assert!(out2.locked_face_sharpness >= 0.0);

        // Clear lock & empty frame -> Idle when tracking disabled
        pipe.clear_lock();
        let out3 = pipe.process_frame(&frame, w, h, false, None);
        assert_eq!(out3.tracking_state, TrackingState::Idle);
    }
}
