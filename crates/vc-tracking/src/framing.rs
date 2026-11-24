//! Anatomical broadcast auto-framing engine.
//!
//! Uses COCO 17-keypoint landmarks (nose, eyes, shoulders, hips, ankles) with graceful bounding-box
//! fallback to compute eye-line anchor errors, joint-safe scale errors for Close-Up / Waist-Up /
//! Full-Height presets, and Rule-of-Thirds composition including hysteresis-stabilized Auto Lead-Room.

use vc_core::{DetectedSubject, FramingConfig, FramingPreset, Keypoint2D, NormalizedRect, RuleOfThirds};

#[derive(Debug, Clone, Copy)]
pub struct FramingError {
    /// Subject anchor position in normalized frame coords `[0..1]`
    pub subject_x: f32,
    pub subject_y: f32,
    /// Current anatomical scale metric (normalized)
    pub subject_scale: f32,
    /// Target framing anchor `[0..1]`
    pub target_x: f32,
    pub target_y: f32,
    /// Target anatomical scale for active preset
    pub target_scale: f32,
    /// Signed pan error (`+` means subject is right of target -> pan right)
    pub err_x: f32,
    /// Signed tilt error (`+` means subject is above target -> tilt up)
    pub err_y: f32,
    /// Signed zoom error (`+` means subject is too small -> zoom in / tele)
    pub err_zoom: f32,
    /// Target quiet/dead-zone rect for UI HUD visualization
    pub deadzone: NormalizedRect,
}

pub struct FramingSolver {
    /// Smoothly slewed horizontal target anchor (prevents sudden jumps in AutoLeadRoom)
    current_target_x: f32,
    /// Lead-room hysteresis timer (-1.0 facing left .. +1.0 facing right)
    facing_accumulator: f32,
    active_lead_side: f32,
}

impl FramingSolver {
    pub fn new() -> Self {
        Self {
            current_target_x: 0.50,
            facing_accumulator: 0.0,
            active_lead_side: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.facing_accumulator = 0.0;
        self.active_lead_side = 0.0;
    }

    pub fn compute(
        &mut self,
        subject: &DetectedSubject,
        config: &FramingConfig,
        subject_vx: f32,
        dt: f32,
    ) -> FramingError {
        let dt = dt.max(1e-3);
        let (subj_x, subj_y, scale, facing_hint) = extract_anatomical_metrics(subject, config.preset);

        // Determine desired horizontal rule-of-thirds target
        let desired_target_x = match config.rule_of_thirds {
            RuleOfThirds::Center => 0.50,
            RuleOfThirds::LeftThird => 0.333,
            RuleOfThirds::RightThird => 0.667,
            RuleOfThirds::AutoLeadRoom => {
                // Fuse gaze orientation hint with horizontal walking direction
                let combined_dir = facing_hint * 0.65 + (subject_vx * 4.0).clamp(-1.0, 1.0) * 0.35;
                if combined_dir > 0.22 {
                    self.facing_accumulator = (self.facing_accumulator + dt).min(1.5);
                } else if combined_dir < -0.22 {
                    self.facing_accumulator = (self.facing_accumulator - dt).max(-1.5);
                } else {
                    self.facing_accumulator *= (1.0 - dt * 0.6).max(0.0);
                }

                // Require ~0.75s sustained facing/walking before flipping lead-room side
                if self.facing_accumulator > 0.75 {
                    self.active_lead_side = 1.0; // looking/walking right -> place on left third
                } else if self.facing_accumulator < -0.75 {
                    self.active_lead_side = -1.0; // looking/walking left -> place on right third
                }

                if self.active_lead_side > 0.5 {
                    0.36
                } else if self.active_lead_side < -0.5 {
                    0.64
                } else {
                    0.50
                }
            }
        };

        // Smoothly slew target_x (max 0.16 frame-width/sec) so preset/lead transitions look cinematic
        let max_slew = 0.16 * dt;
        let diff_tx = (desired_target_x - self.current_target_x).clamp(-max_slew, max_slew);
        self.current_target_x += diff_tx;

        let (target_y, target_scale) = preset_targets(config.preset);

        // Positive err_x: subject is to the right of target -> pan right (+)
        let err_x = subj_x - self.current_target_x;
        // Image Y grows downward, whereas PTZ tilt + is UP:
        // If subject eye_y < target_y (higher in frame), camera must tilt UP (+)!
        let err_y = target_y - subj_y;
        // Relative zoom error: + if subject is smaller than target scale (zoom in / tele)
        let err_zoom = (target_scale - scale) / target_scale.max(0.05);

        let deadzone = NormalizedRect {
            x: (self.current_target_x - config.deadzone_x).clamp(0.0, 1.0),
            y: (target_y - config.deadzone_y).clamp(0.0, 1.0),
            width: (config.deadzone_x * 2.0).min(1.0),
            height: (config.deadzone_y * 2.0).min(1.0),
        };

        FramingError {
            subject_x: subj_x,
            subject_y: subj_y,
            subject_scale: scale,
            target_x: self.current_target_x,
            target_y,
            target_scale,
            err_x,
            err_y,
            err_zoom,
            deadzone,
        }
    }

    pub fn current_anchor(&self, preset: FramingPreset) -> [f32; 2] {
        let (ty, _) = preset_targets(preset);
        [self.current_target_x, ty]
    }
}

impl Default for FramingSolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Target eye-line vertical coordinate and target anatomical scale metric per broadcast shot preset.
pub fn preset_targets(preset: FramingPreset) -> (f32, f32) {
    match preset {
        // Close-Up (Head & Shoulders): eyes right on upper third (33%), shoulder/head scale larger
        FramingPreset::CloseUp => (0.333, 0.78),
        // Waist-Up (Medium Broadcast): eyes around 28%, cuts cleanly just below waist/hips
        FramingPreset::WaistUp => (0.28, 0.56),
        // Full Height: natural 12% headroom (eyes ~17%), full head-to-feet body ~78% height
        FramingPreset::FullHeight => (0.17, 0.78),
    }
}

/// Extract `(eye_anchor_x, eye_anchor_y, shot_scale_metric, facing_direction)` from keypoints
/// (immune to raised hands / holding microphones) with smooth bounding-box fallback.
fn extract_anatomical_metrics(subject: &DetectedSubject, preset: FramingPreset) -> (f32, f32, f32, f32) {
    let b = &subject.bbox;
    let kp = |idx: usize| -> Option<&Keypoint2D> {
        subject.keypoints.get(idx).filter(|k| k.score > 0.35)
    };

    // Standard COCO-17 indices (or our 9-keypoint demo subset):
    // 0: nose, 1: l_eye, 2: r_eye, 3/5: l_shoulder, 4/6: r_shoulder
    let nose = kp(0);
    let l_eye = kp(1);
    let r_eye = kp(2);

    // Support both full COCO-17 (shoulders at 5,6; hips at 11,12; ankles at 15,16)
    // and compact 9-keypoint layout (shoulders at 3,4; hips at 5,6; ankles at 7,8)
    let is_coco17 = subject.keypoints.len() >= 17;
    let (l_sh, r_sh, l_hip, r_hip, l_ank, r_ank) = if is_coco17 {
        (kp(5), kp(6), kp(11), kp(12), kp(15), kp(16))
    } else {
        (kp(3), kp(4), kp(5), kp(6), kp(7), kp(8))
    };

    // Eye/head anchor position
    let (anchor_x, anchor_y) = match (l_eye, r_eye, nose) {
        (Some(le), Some(re), _) => ((le.x + re.x) * 0.5, (le.y + re.y) * 0.5),
        (_, _, Some(n)) => (n.x, n.y - b.height * 0.02),
        _ => (b.x + b.width * 0.5, b.y + b.height * 0.16),
    };

    // Facing direction hint (-1 left .. +1 right) from nose relative to shoulder center
    let facing_hint = match (nose, l_sh, r_sh) {
        (Some(n), Some(ls), Some(rs)) => {
            let sh_cx = (ls.x + rs.x) * 0.5;
            let sh_w = (ls.x - rs.x).abs().max(0.03);
            ((n.x - sh_cx) / (sh_w * 0.45)).clamp(-1.0, 1.0)
        }
        _ => 0.0,
    };

    // Preset-specific anatomical scale (avoids jumping when arms are raised)
    let scale = match preset {
        FramingPreset::CloseUp => {
            // Shoulder width × 2.2 approximates close-up coverage cleanly
            if let (Some(ls), Some(rs)) = (l_sh, r_sh) {
                let sh_w = (ls.x - rs.x).abs();
                let torso_h = match (l_hip, r_hip) {
                    (Some(lh), Some(rh)) => (((lh.y + rh.y) * 0.5) - ((ls.y + rs.y) * 0.5)).abs(),
                    _ => sh_w * 1.4,
                };
                (sh_w * 2.2).max(torso_h * 1.9)
            } else {
                b.height
            }
        }
        FramingPreset::WaistUp => {
            // Eye-to-hip span is rock-solid even when speaker gestures with hands
            if let (Some(lh), Some(rh)) = (l_hip, r_hip) {
                let hip_y = (lh.y + rh.y) * 0.5;
                ((hip_y - anchor_y).abs() * 1.15).max(b.height * 0.55)
            } else {
                b.height * 0.85
            }
        }
        FramingPreset::FullHeight => {
            // Eye-to-ankle span + headroom margin
            if let (Some(la), Some(ra)) = (l_ank, r_ank) {
                let ank_y = la.y.max(ra.y);
                ((ank_y - anchor_y).abs() * 1.12).max(b.height * 0.85)
            } else {
                b.height
            }
        }
    };

    (anchor_x, anchor_y, scale.clamp(0.05, 2.0), facing_hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject_at(cx: f32, eye_y: f32, height: f32) -> DetectedSubject {
        DetectedSubject {
            track_id: 1,
            bbox: NormalizedRect {
                x: cx - height * 0.15,
                y: eye_y - height * 0.15,
                width: height * 0.30,
                height,
            },
            confidence: 0.95,
            is_locked: true,
            identity_similarity: Some(0.95),
            keypoints: vec![
                Keypoint2D { x: cx, y: eye_y + 0.01, score: 0.95 },
                Keypoint2D { x: cx - 0.015, y: eye_y, score: 0.95 },
                Keypoint2D { x: cx + 0.015, y: eye_y, score: 0.95 },
                Keypoint2D { x: cx - 0.06, y: eye_y + height * 0.18, score: 0.95 },
                Keypoint2D { x: cx + 0.06, y: eye_y + height * 0.18, score: 0.95 },
                Keypoint2D { x: cx - 0.04, y: eye_y + height * 0.52, score: 0.92 },
                Keypoint2D { x: cx + 0.04, y: eye_y + height * 0.52, score: 0.92 },
                Keypoint2D { x: cx - 0.03, y: eye_y + height * 0.88, score: 0.90 },
                Keypoint2D { x: cx + 0.03, y: eye_y + height * 0.88, score: 0.90 },
            ],
            face_sharpness: Some(0.85),
        }
    }

    #[test]
    fn rule_of_thirds_presets_drive_correct_pan_tilt_signs() {
        let mut solver = FramingSolver::new();
        let mut cfg = FramingConfig::default();
        cfg.rule_of_thirds = RuleOfThirds::LeftThird;
        cfg.preset = FramingPreset::CloseUp;

        // Let target_x slew onto LeftThird (0.333)
        let s = subject_at(0.65, 0.20, 0.45);
        let mut err = solver.compute(&s, &cfg, 0.0, 0.033);
        for _ in 0..60 {
            err = solver.compute(&s, &cfg, 0.0, 0.033);
        }
        assert!((err.target_x - 0.333).abs() < 0.01);
        assert!(err.err_x > 0.25, "subject right of left-third must request positive pan right");
        assert!(err.err_y > 0.10, "subject higher than upper-third must request positive tilt up");
        assert!(err.err_zoom > 0.20, "small subject in close-up preset must request zoom in");
    }

    #[test]
    fn auto_lead_room_places_rightward_walker_on_left_third() {
        let mut solver = FramingSolver::new();
        let mut cfg = FramingConfig::default();
        cfg.rule_of_thirds = RuleOfThirds::AutoLeadRoom;

        let s = subject_at(0.50, 0.28, 0.55);
        let mut err = solver.compute(&s, &cfg, 0.18, 0.033);
        for _ in 0..90 {
            err = solver.compute(&s, &cfg, 0.18, 0.033);
        }
        assert!(
            err.target_x < 0.40,
            "walking right should shift framing target to left third (~0.36), got {:.3}",
            err.target_x
        );
    }

    #[test]
    fn covers_all_presets_third_modes_coco17_and_bbox_fallbacks() {
        let mut solver = FramingSolver::default();
        let mut cfg = FramingConfig::default();

        // Walk left in AutoLeadRoom -> places subject on right third (~0.64)
        cfg.rule_of_thirds = RuleOfThirds::AutoLeadRoom;
        let s = subject_at(0.50, 0.28, 0.55);
        let mut err = solver.compute(&s, &cfg, -0.25, 0.05);
        for _ in 0..80 {
            err = solver.compute(&s, &cfg, -0.25, 0.05);
        }
        assert!(err.target_x > 0.60);

        // Idle decay branch + reset + Center & RightThird
        let _ = solver.compute(&s, &cfg, 0.0, 0.05);
        solver.reset();
        cfg.rule_of_thirds = RuleOfThirds::Center;
        let _ = solver.compute(&s, &cfg, 0.0, 0.5);
        cfg.rule_of_thirds = RuleOfThirds::RightThird;
        let _ = solver.compute(&s, &cfg, 0.0, 0.5);
        let anchor = solver.current_anchor(FramingPreset::FullHeight);
        assert!((anchor[1] - 0.17).abs() < 1e-4);

        // FullHeight & WaistUp with 17-keypoint COCO layout
        let mut coco17 = s.clone();
        coco17.keypoints = vec![Keypoint2D { x: 0.5, y: 0.3, score: 0.9 }; 17];
        coco17.keypoints[5] = Keypoint2D { x: 0.44, y: 0.38, score: 0.9 };
        coco17.keypoints[6] = Keypoint2D { x: 0.56, y: 0.38, score: 0.9 };
        coco17.keypoints[11] = Keypoint2D { x: 0.46, y: 0.58, score: 0.9 };
        coco17.keypoints[12] = Keypoint2D { x: 0.54, y: 0.58, score: 0.9 };
        coco17.keypoints[15] = Keypoint2D { x: 0.47, y: 0.85, score: 0.9 };
        coco17.keypoints[16] = Keypoint2D { x: 0.53, y: 0.85, score: 0.9 };

        for preset in [FramingPreset::CloseUp, FramingPreset::WaistUp, FramingPreset::FullHeight] {
            cfg.preset = preset;
            let e = solver.compute(&coco17, &cfg, 0.0, 0.033);
            assert!(e.subject_scale > 0.05);
        }

        // Nose-only keypoint + CloseUp with shoulders only (no hips)
        let mut nose_only = s.clone();
        nose_only.keypoints = vec![
            Keypoint2D { x: 0.5, y: 0.25, score: 0.9 },
            Keypoint2D { x: 0.0, y: 0.0, score: 0.1 },
            Keypoint2D { x: 0.0, y: 0.0, score: 0.1 },
            Keypoint2D { x: 0.42, y: 0.35, score: 0.9 },
            Keypoint2D { x: 0.58, y: 0.35, score: 0.9 },
        ];
        cfg.preset = FramingPreset::CloseUp;
        let _ = solver.compute(&nose_only, &cfg, 0.0, 0.033);

        // No keypoints at all -> bbox fallback across all 3 presets
        let mut no_kps = s.clone();
        no_kps.keypoints.clear();
        for preset in [FramingPreset::CloseUp, FramingPreset::WaistUp, FramingPreset::FullHeight] {
            cfg.preset = preset;
            let e = solver.compute(&no_kps, &cfg, 0.0, 0.033);
            assert!(e.subject_scale > 0.05);
        }
    }
}
