//! BoT-SORT multi-object tracker with Camera Motion Compensation (CMC),
//! two-stage confidence association, and overlap-protected operator identity gating.

use stagelock_core::{DetectedSubject, Keypoint2D, NormalizedRect, TrackingState};

use crate::reid::{AppearanceEmbedding, IdentityGallery};

#[derive(Debug, Clone)]
pub struct RawDetection {
    pub bbox: NormalizedRect,
    pub confidence: f32,
    pub keypoints: Vec<Keypoint2D>,
    pub embedding: Option<AppearanceEmbedding>,
}

#[derive(Debug, Clone)]
pub struct TrackingSnapshot {
    pub subjects: Vec<DetectedSubject>,
    pub tracking_state: TrackingState,
    pub locked_track_id: Option<u64>,
    pub locked_similarity: Option<f32>,
    pub overlap_frozen: bool,
}

#[derive(Debug, Clone)]
struct Track {
    id: u64,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    vx: f32,
    vy: f32,
    vw: f32,
    vh: f32,
    confidence: f32,
    keypoints: Vec<Keypoint2D>,
    embedding: Option<AppearanceEmbedding>,
    face_sharpness: Option<f32>,
    hits: u32,
    missed: u32,
}

impl Track {
    fn from_det(id: u64, det: RawDetection, sharpness: Option<f32>) -> Self {
        Self {
            id,
            cx: det.bbox.x + det.bbox.width * 0.5,
            cy: det.bbox.y + det.bbox.height * 0.5,
            w: det.bbox.width,
            h: det.bbox.height,
            vx: 0.0,
            vy: 0.0,
            vw: 0.0,
            vh: 0.0,
            confidence: det.confidence,
            keypoints: det.keypoints,
            embedding: det.embedding,
            face_sharpness: sharpness,
            hits: 1,
            missed: 0,
        }
    }

    fn bbox(&self) -> NormalizedRect {
        NormalizedRect {
            x: (self.cx - self.w * 0.5).clamp(0.0, 1.0),
            y: (self.cy - self.h * 0.5).clamp(0.0, 1.0),
            width: self.w.clamp(0.01, 1.0),
            height: self.h.clamp(0.01, 1.0),
        }
    }

    fn predict(&mut self, cmc_dx: f32, cmc_dy: f32) {
        // Apply Camera Motion Compensation + damped constant-velocity prediction
        self.cx = (self.cx + self.vx + cmc_dx).clamp(0.0, 1.0);
        self.cy = (self.cy + self.vy + cmc_dy).clamp(0.0, 1.0);
        self.w = (self.w + self.vw * 0.5).clamp(0.02, 1.0);
        self.h = (self.h + self.vh * 0.5).clamp(0.02, 1.0);
        // Gentle velocity damping during occlusion so box doesn't drift off-screen
        self.vx *= 0.85;
        self.vy *= 0.85;
        self.vw *= 0.7;
        self.vh *= 0.7;
    }

    fn update(&mut self, det: &RawDetection, sharpness: Option<f32>, allow_embedding_update: bool) {
        let new_cx = det.bbox.x + det.bbox.width * 0.5;
        let new_cy = det.bbox.y + det.bbox.height * 0.5;
        let new_w = det.bbox.width;
        let new_h = det.bbox.height;

        if self.missed >= 3 {
            // Re-acquired after occlusion: snap directly to fresh observation and reset stale velocity
            self.cx = new_cx;
            self.cy = new_cy;
            self.w = new_w;
            self.h = new_h;
            self.vx = 0.0;
            self.vy = 0.0;
            self.vw = 0.0;
            self.vh = 0.0;
        } else {
            let alpha_pos = 0.65;
            let beta_vel = 0.35;
            let dx = new_cx - self.cx;
            let dy = new_cy - self.cy;
            let dw = new_w - self.w;
            let dh = new_h - self.h;

            self.cx += alpha_pos * dx;
            self.cy += alpha_pos * dy;
            self.w += alpha_pos * dw;
            self.h += alpha_pos * dh;

            self.vx = (1.0 - beta_vel) * self.vx + beta_vel * dx;
            self.vy = (1.0 - beta_vel) * self.vy + beta_vel * dy;
            self.vw = (1.0 - beta_vel) * self.vw + beta_vel * dw;
            self.vh = (1.0 - beta_vel) * self.vh + beta_vel * dh;
        }

        self.confidence = det.confidence;
        if !det.keypoints.is_empty() {
            self.keypoints = det.keypoints.clone();
        }
        if let Some(s) = sharpness {
            self.face_sharpness = Some(s);
        }
        if allow_embedding_update {
            if let Some(ref emb) = det.embedding {
                match self.embedding.as_mut() {
                    Some(existing) => existing.blend_ema(emb, 0.15),
                    None => self.embedding = Some(emb.clone()),
                }
            }
        }
        self.hits = self.hits.saturating_add(1);
        self.missed = 0;
    }
}

struct LockedIdentity {
    active_track_id: Option<u64>,
    gallery: IdentityGallery,
    missing_frames: u32,
}

pub struct BotSortTracker {
    next_id: u64,
    tracks: Vec<Track>,
    locked: Option<LockedIdentity>,
    max_track_age: u32,
    occlusion_hold_frames: u32,
    reid_reacquire_threshold: f32,
}

impl BotSortTracker {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            tracks: Vec::new(),
            locked: None,
            max_track_age: 45,
            occlusion_hold_frames: 75, // ~2.5s at 30fps
            reid_reacquire_threshold: 0.78,
        }
    }

    pub fn clear_lock(&mut self) {
        self.locked = None;
    }

    pub fn lock_track(&mut self, track_id: u64) -> Option<u64> {
        let track = self.tracks.iter().find(|t| t.id == track_id)?;
        let emb = track
            .embedding
            .clone()
            .unwrap_or_else(|| AppearanceEmbedding::from_raw(vec![1.0]));
        self.locked = Some(LockedIdentity {
            active_track_id: Some(track.id),
            gallery: IdentityGallery::new(emb, 12),
            missing_frames: 0,
        });
        Some(track.id)
    }

    pub fn lock_at_point(&mut self, norm_x: f32, norm_y: f32) -> Option<u64> {
        // Prefer smallest bounding box containing the click/tap point for tight selection
        let chosen = self
            .tracks
            .iter()
            .filter(|t| t.missed == 0)
            .filter(|t| {
                let b = t.bbox();
                norm_x >= b.x
                    && norm_x <= b.x + b.width
                    && norm_y >= b.y
                    && norm_y <= b.y + b.height
            })
            .min_by(|a, b| {
                let area_a = a.w * a.h;
                let area_b = b.w * b.h;
                area_a.total_cmp(&area_b)
            })
            .map(|t| t.id)?;

        self.lock_track(chosen)
    }

    /// Run one BoT-SORT tracking step with Camera Motion Compensation `(cmc_dx, cmc_dy)`.
    pub fn step(
        &mut self,
        detections: Vec<RawDetection>,
        sharpness_per_det: &[Option<f32>],
        cmc_dx: f32,
        cmc_dy: f32,
        auto_track_enabled: bool,
    ) -> TrackingSnapshot {
        // 1. Predict all existing tracks with CMC shift
        for t in &mut self.tracks {
            t.predict(cmc_dx, cmc_dy);
        }

        // Partition detections into high-conf (Stage 1) and low-conf (Stage 2)
        let mut high_idx = Vec::new();
        let mut low_idx = Vec::new();
        for (i, d) in detections.iter().enumerate() {
            if d.confidence >= 0.45 {
                high_idx.push(i);
            } else if d.confidence >= 0.15 {
                low_idx.push(i);
            }
        }

        let mut matched_tracks = vec![false; self.tracks.len()];
        let mut matched_dets = vec![false; detections.len()];

        // 2. Stage 1: Fused IoU + Re-ID cosine similarity association
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            let tb = t.bbox();
            for &di in &high_idx {
                let det = &detections[di];
                let iou = rect_iou(&tb, &det.bbox);
                let emb_sim = match (&t.embedding, &det.embedding) {
                    (Some(a), Some(b)) => Some(a.cosine_similarity(b)),
                    _ => None,
                };

                // Hard anti-swap gate: never associate if appearance clearly disagrees
                if let Some(sim) = emb_sim {
                    if sim < 0.58 {
                        continue;
                    }
                    if iou < 0.08 && sim < 0.80 {
                        continue;
                    }
                    let cost = 0.50 * (1.0 - iou) + 0.50 * (1.0 - sim);
                    if cost < 0.65 {
                        pairs.push((cost, ti, di));
                    }
                } else if iou >= 0.20 {
                    pairs.push((1.0 - iou, ti, di));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

        // Precompute detection overlaps so crowded detections don't corrupt track embeddings
        let det_overlapping: Vec<bool> = detections
            .iter()
            .enumerate()
            .map(|(i, a)| {
                detections
                    .iter()
                    .enumerate()
                    .any(|(j, b)| i != j && rect_iou(&a.bbox, &b.bbox) > 0.15)
            })
            .collect();

        for (_, ti, di) in pairs {
            if matched_tracks[ti] || matched_dets[di] {
                continue;
            }
            matched_tracks[ti] = true;
            matched_dets[di] = true;
            let sharp = sharpness_per_det.get(di).copied().flatten();
            self.tracks[ti].update(&detections[di], sharp, !det_overlapping[di]);
        }

        // 3. Stage 2: Low-confidence detection recovery via IoU for remaining active tracks
        let mut low_pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            if matched_tracks[ti] || t.missed > 2 {
                continue;
            }
            let tb = t.bbox();
            for &di in &low_idx {
                if matched_dets[di] {
                    continue;
                }
                let iou = rect_iou(&tb, &detections[di].bbox);
                if iou >= 0.25 {
                    low_pairs.push((1.0 - iou, ti, di));
                }
            }
        }
        low_pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

        for (_, ti, di) in low_pairs {
            if matched_tracks[ti] || matched_dets[di] {
                continue;
            }
            matched_tracks[ti] = true;
            matched_dets[di] = true;
            let sharp = sharpness_per_det.get(di).copied().flatten();
            // Do not update appearance embedding from low-confidence occluded detections
            self.tracks[ti].update(&detections[di], sharp, false);
        }

        // 4. Age unmatched tracks & spawn new tracks for unmatched high-conf detections
        for (ti, t) in self.tracks.iter_mut().enumerate() {
            if !matched_tracks[ti] {
                t.missed = t.missed.saturating_add(1);
            }
        }
        self.tracks.retain(|t| t.missed <= self.max_track_age);

        for &di in &high_idx {
            if matched_dets[di] {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            let sharp = sharpness_per_det.get(di).copied().flatten();
            self.tracks.push(Track::from_det(id, detections[di].clone(), sharp));
        }

        // 5. Operator Identity Gate + Overlap Freeze + Seamless Re-Acquisition
        let mut overlap_frozen = false;
        let mut locked_similarity = None;

        let tracking_state = if let Some(ref mut lock) = self.locked {
            // Check if currently locked track is visible this frame
            let active_visible = lock.active_track_id.and_then(|tid| {
                self.tracks
                    .iter()
                    .find(|t| t.id == tid && t.missed == 0)
            });

            if let Some(locked_track) = active_visible {
                let lb = locked_track.bbox();
                overlap_frozen = self.tracks.iter().any(|other| {
                    other.id != locked_track.id
                        && other.missed == 0
                        && rect_iou(&lb, &other.bbox()) > 0.15
                });

                if let Some(ref emb) = locked_track.embedding {
                    let sim = lock.gallery.similarity(emb);
                    locked_similarity = Some(sim);
                    // Guard against silent takeover even if boxes overlapped tightly:
                    // if isolated and appearance suddenly diverges wildly, drop to OccludedHold
                    if !overlap_frozen && sim < 0.52 {
                        lock.active_track_id = None;
                        lock.missing_frames = lock.missing_frames.saturating_add(1);
                    } else {
                        lock.missing_frames = 0;
                        if !overlap_frozen && locked_track.confidence >= 0.55 {
                            lock.gallery.update_isolated(emb.clone());
                        }
                    }
                } else {
                    lock.missing_frames = 0;
                }
            } else {
                // Target occluded or temporarily exited: try Re-ID re-acquisition across visible tracks
                lock.active_track_id = None;
                lock.missing_frames = lock.missing_frames.saturating_add(1);

                let mut candidates: Vec<(f32, u64)> = self
                    .tracks
                    .iter()
                    .filter(|t| t.missed == 0 && t.confidence >= 0.45)
                    .filter_map(|t| {
                        let sim = t.embedding.as_ref().map(|e| lock.gallery.similarity(e))?;
                        Some((sim, t.id))
                    })
                    .collect();
                candidates.sort_by(|a, b| b.0.total_cmp(&a.0));

                if let Some(&(best_sim, best_id)) = candidates.first() {
                    let second_sim = candidates.get(1).map(|c| c.0).unwrap_or(0.0);
                    let margin_ok = candidates.len() == 1 || (best_sim - second_sim) >= 0.08;
                    if best_sim >= self.reid_reacquire_threshold && margin_ok {
                        lock.active_track_id = Some(best_id);
                        lock.missing_frames = 0;
                        locked_similarity = Some(best_sim);
                    }
                }
            }

            if !auto_track_enabled {
                TrackingState::Idle
            } else if lock.active_track_id.is_some() {
                TrackingState::Locked
            } else if lock.missing_frames <= self.occlusion_hold_frames {
                TrackingState::OccludedHold
            } else {
                TrackingState::Searching
            }
        } else if auto_track_enabled {
            TrackingState::Searching
        } else {
            TrackingState::Idle
        };

        let locked_track_id = self.locked.as_ref().and_then(|l| l.active_track_id);

        let subjects: Vec<DetectedSubject> = self
            .tracks
            .iter()
            .filter(|t| t.missed == 0)
            .map(|t| {
                let is_locked = locked_track_id == Some(t.id);
                let sim = self.locked.as_ref().and_then(|l| {
                    t.embedding.as_ref().map(|e| l.gallery.similarity(e))
                });
                DetectedSubject {
                    track_id: t.id,
                    bbox: t.bbox(),
                    confidence: t.confidence,
                    is_locked,
                    identity_similarity: sim,
                    keypoints: t.keypoints.clone(),
                    face_sharpness: t.face_sharpness,
                }
            })
            .collect();

        TrackingSnapshot {
            subjects,
            tracking_state,
            locked_track_id,
            locked_similarity,
            overlap_frozen,
        }
    }
}

impl Default for BotSortTracker {
    fn default() -> Self {
        Self::new()
    }
}

pub fn rect_iou(a: &NormalizedRect, b: &NormalizedRect) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    if inter <= 0.0 {
        return 0.0;
    }
    let area_a = a.width * a.height;
    let area_b = b.width * b.height;
    inter / (area_a + area_b - inter).max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_emb(primary_dim: usize, noise: f32) -> AppearanceEmbedding {
        let mut v = vec![noise; 16];
        v[primary_dim] = 1.0;
        AppearanceEmbedding::from_raw(v)
    }

    fn det(cx: f32, cy: f32, emb: AppearanceEmbedding) -> RawDetection {
        RawDetection {
            bbox: NormalizedRect {
                x: cx - 0.07,
                y: cy - 0.25,
                width: 0.14,
                height: 0.50,
            },
            confidence: 0.92,
            keypoints: Vec::new(),
            embedding: Some(emb),
        }
    }

    #[test]
    fn crossing_subjects_maintain_identity_lock_without_id_swap() {
        let mut tracker = BotSortTracker::new();
        let emb_a = make_emb(0, 0.02);
        let emb_b = make_emb(7, 0.02);

        // Frame 0: Person A on left (0.20), Person B on right (0.80)
        let snap0 = tracker.step(
            vec![det(0.20, 0.50, emb_a.clone()), det(0.80, 0.50, emb_b.clone())],
            &[None, None],
            0.0,
            0.0,
            true,
        );
        let id_a = snap0.subjects.iter().find(|s| s.bbox.x < 0.30).unwrap().track_id;
        assert_eq!(tracker.lock_track(id_a), Some(id_a));

        // Walk Person A left->right and Person B right->left straight through full overlap!
        let mut last_snap = snap0;
        for step_i in 1..=30 {
            let t = step_i as f32 / 30.0;
            let ax = 0.20 + 0.60 * t;
            let bx = 0.80 - 0.60 * t;
            last_snap = tracker.step(
                vec![det(ax, 0.50, emb_a.clone()), det(bx, 0.50, emb_b.clone())],
                &[None, None],
                0.0,
                0.0,
                true,
            );
        }

        // At the end, Person A is at x ≈ 0.80 and MUST still be the locked subject!
        assert_eq!(last_snap.tracking_state, TrackingState::Locked);
        let locked_subj = last_snap.subjects.iter().find(|s| s.is_locked).unwrap();
        let locked_cx = locked_subj.bbox.x + locked_subj.bbox.width * 0.5;
        assert!(
            locked_cx > 0.72,
            "lock must follow Person A to right side (~0.80), got {locked_cx:.3}"
        );
    }

    #[test]
    fn complete_occlusion_holds_and_reacquires_exact_person_not_bystander() {
        let mut tracker = BotSortTracker::new();
        let emb_a = make_emb(2, 0.01);
        let emb_b = make_emb(11, 0.01);

        // Lock onto Person A
        let s0 = tracker.step(
            vec![det(0.30, 0.50, emb_a.clone()), det(0.65, 0.50, emb_b.clone())],
            &[None, None],
            0.0,
            0.0,
            true,
        );
        let id_a = s0.subjects.iter().find(|s| s.bbox.x < 0.40).unwrap().track_id;
        tracker.lock_track(id_a);

        // Person A vanishes behind a stage wall for 20 frames while Person B walks right across
        for f in 0..20 {
            let bx = 0.65 - 0.015 * f as f32;
            let snap = tracker.step(vec![det(bx, 0.50, emb_b.clone())], &[None], 0.0, 0.0, true);
            assert_eq!(
                snap.tracking_state,
                TrackingState::OccludedHold,
                "must hold calmly during occlusion without jumping to Person B"
            );
            assert!(snap.locked_track_id.is_none());
        }

        // Person A re-emerges far across stage at x = 0.78 (new bbox location!)
        let reacquired = tracker.step(
            vec![det(0.35, 0.50, emb_b.clone()), det(0.78, 0.50, emb_a.clone())],
            &[None, None],
            0.0,
            0.0,
            true,
        );
        assert_eq!(reacquired.tracking_state, TrackingState::Locked);
        let locked_subj = reacquired.subjects.iter().find(|s| s.is_locked).unwrap();
        let locked_cx = locked_subj.bbox.x + locked_subj.bbox.width * 0.5;
        assert!(
            locked_cx > 0.70,
            "must re-acquire Person A at ~0.78 rather than bystander B at ~0.35, got {locked_cx:.3}"
        );
    }

    #[test]
    fn camera_motion_compensation_preserves_track_during_fast_pan() {
        let mut tracker = BotSortTracker::new();
        let emb = make_emb(4, 0.02);

        let s0 = tracker.step(vec![det(0.50, 0.50, emb.clone())], &[None], 0.0, 0.0, true);
        let tid = s0.subjects[0].track_id;

        // Fast camera pan shifts everything right by +0.16 in a single frame
        let s1 = tracker.step(vec![det(0.66, 0.50, emb.clone())], &[None], 0.16, 0.0, true);
        assert_eq!(s1.subjects.len(), 1);
        assert_eq!(s1.subjects[0].track_id, tid, "CMC should keep unbroken track ID during fast pan");
    }

    #[test]
    fn lock_unknown_id_and_empty_point_return_none() {
        let mut tracker = BotSortTracker::new();
        let emb = make_emb(1, 0.02);
        let _ = tracker.step(vec![det(0.25, 0.50, emb)], &[None], 0.0, 0.0, true);

        assert!(tracker.lock_track(9999).is_none());
        // Click far outside person at x=0.25
        assert!(tracker.lock_at_point(0.90, 0.90).is_none());
    }

    #[test]
    fn prolonged_occlusion_expires_into_searching_without_hijacking_bystander() {
        let mut tracker = BotSortTracker::new();
        let emb_a = make_emb(0, 0.01);
        let emb_b = make_emb(12, 0.01);

        let s0 = tracker.step(
            vec![det(0.25, 0.50, emb_a), det(0.75, 0.50, emb_b.clone())],
            &[None, None],
            0.0,
            0.0,
            true,
        );
        let id_a = s0.subjects.iter().find(|s| s.bbox.x < 0.40).unwrap().track_id;
        assert_eq!(tracker.lock_track(id_a), Some(id_a));

        // Exceed occlusion_hold_frames (75) with only bystander B visible
        let mut last = s0;
        for _ in 0..80 {
            last = tracker.step(vec![det(0.75, 0.50, emb_b.clone())], &[None], 0.0, 0.0, true);
            assert!(last.locked_track_id.is_none(), "must never hijack bystander B");
        }
        assert_eq!(
            last.tracking_state,
            TrackingState::Searching,
            "after occlusion hold expires, tracker must transition to Searching"
        );
    }

    #[test]
    fn empty_detections_and_noise_below_threshold_spawn_no_tracks() {
        let mut tracker = BotSortTracker::new();
        let noise = RawDetection {
            bbox: NormalizedRect { x: 0.1, y: 0.1, width: 0.2, height: 0.3 },
            confidence: 0.05, // below 0.15 cutoff
            keypoints: Vec::new(),
            embedding: None,
        };
        let snap1 = tracker.step(vec![noise], &[None], 0.0, 0.0, true);
        assert!(snap1.subjects.is_empty());
        assert_eq!(snap1.tracking_state, TrackingState::Searching);

        let snap2 = tracker.step(Vec::new(), &[], 0.0, 0.0, false);
        assert!(snap2.subjects.is_empty());
        assert_eq!(snap2.tracking_state, TrackingState::Idle);
    }
}
