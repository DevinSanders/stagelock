//! Body appearance Re-Identification (Re-ID) descriptors and multi-angle adaptive identity gallery.
//!
//! Supports both:
//! - Neural 512-dim OSNet embeddings (when `models/osnet_x0_25.onnx` is loaded)
//! - Fast 96-dim spatial 3-zone (head / upper torso / lower body) HSV appearance descriptors
//!   as a zero-dependency fallback so multi-person identity gating works immediately.

use stagelock_core::NormalizedRect;

#[derive(Debug, Clone)]
pub struct AppearanceEmbedding {
    pub values: Vec<f32>,
}

impl AppearanceEmbedding {
    pub fn from_raw(mut values: Vec<f32>) -> Self {
        let norm: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in &mut values {
                *v /= norm;
            }
        }
        Self { values }
    }

    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        if self.values.len() != other.values.len() || self.values.is_empty() {
            return 0.0;
        }
        self.values
            .iter()
            .zip(other.values.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>()
            .clamp(-1.0, 1.0)
    }

    pub fn blend_ema(&mut self, other: &Self, alpha: f32) {
        if self.values.len() != other.values.len() {
            return;
        }
        let keep = 1.0 - alpha;
        for (a, b) in self.values.iter_mut().zip(other.values.iter()) {
            *a = *a * keep + *b * alpha;
        }
        let norm: f32 = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in &mut self.values {
                *v /= norm;
            }
        }
    }
}

/// Extract an L2-normalized 96-dim spatial appearance descriptor (3 vertical body bands ×
/// 16 hue + 8 saturation + 8 value bins) from the inner 70% of a person's bounding box.
pub fn extract_spatial_hsv_descriptor(
    bgra: &[u8],
    width: u32,
    height: u32,
    bbox: &NormalizedRect,
) -> AppearanceEmbedding {
    const BANDS: usize = 3;
    const H_BINS: usize = 16;
    const S_BINS: usize = 8;
    const V_BINS: usize = 8;
    const BAND_DIM: usize = H_BINS + S_BINS + V_BINS;
    let mut hist = vec![0.0_f32; BANDS * BAND_DIM];

    if width < 4 || height < 4 || bgra.len() < (width * height * 4) as usize {
        return AppearanceEmbedding::from_raw(hist);
    }

    // Inset horizontally by 15% on each side so background edges don't pollute clothing color
    let x0 = (((bbox.x + bbox.width * 0.15) * width as f32).floor() as usize).min(width as usize - 1);
    let x1 = (((bbox.x + bbox.width * 0.85) * width as f32).ceil() as usize).clamp(x0 + 1, width as usize);
    let y0 = ((bbox.y * height as f32).floor() as usize).min(height as usize - 1);
    let y1 = (((bbox.y + bbox.height) * height as f32).ceil() as usize).clamp(y0 + 1, height as usize);

    let w = width as usize;
    let bh = (y1 - y0).max(1);
    let step_x = ((x1 - x0) / 24).max(1);
    let step_y = (bh / 36).max(1);

    let mut y = y0;
    while y < y1 {
        let band = (((y - y0) * BANDS) / bh).min(BANDS - 1);
        let base = band * BAND_DIM;
        let mut x = x0;
        while x < x1 {
            let i = (y * w + x) * 4;
            let (h, s, v) = bgr_to_hsv(bgra[i], bgra[i + 1], bgra[i + 2]);
            let h_bin = ((h * H_BINS as f32) as usize).min(H_BINS - 1);
            let s_bin = ((s * S_BINS as f32) as usize).min(S_BINS - 1);
            let v_bin = ((v * V_BINS as f32) as usize).min(V_BINS - 1);
            // Weight hue by saturation so neutral grays don't bias random hue bins
            hist[base + h_bin] += 0.25 + s * 1.5;
            hist[base + H_BINS + s_bin] += 0.7;
            hist[base + H_BINS + S_BINS + v_bin] += 0.5;
            x += step_x;
        }
        y += step_y;
    }

    AppearanceEmbedding::from_raw(hist)
}

fn bgr_to_hsv(b: u8, g: u8, r: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let h = if delta < 1e-5 {
        0.0
    } else if (max - rf).abs() < 1e-5 {
        (((gf - bf) / delta) % 6.0) / 6.0
    } else if (max - gf).abs() < 1e-5 {
        (((bf - rf) / delta) + 2.0) / 6.0
    } else {
        (((rf - gf) / delta) + 4.0) / 6.0
    };
    let h = if h < 0.0 { h + 1.0 } else { h };
    let s = if max < 1e-5 { 0.0 } else { delta / max };
    (h.clamp(0.0, 0.9999), s.clamp(0.0, 1.0), max.clamp(0.0, 1.0))
}

/// Rolling multi-angle appearance gallery for an operator-locked target.
/// Holds up to `max_entries` diverse views (front, side, back) and freezes updates during overlaps.
#[derive(Debug, Clone)]
pub struct IdentityGallery {
    entries: Vec<AppearanceEmbedding>,
    max_entries: usize,
}

impl IdentityGallery {
    pub fn new(initial: AppearanceEmbedding, max_entries: usize) -> Self {
        Self {
            entries: vec![initial],
            max_entries: max_entries.max(2),
        }
    }

    pub fn similarity(&self, candidate: &AppearanceEmbedding) -> f32 {
        self.entries
            .iter()
            .map(|e| e.cosine_similarity(candidate))
            .fold(0.0_f32, f32::max)
    }

    /// Update gallery ONLY when the locked person is cleanly isolated (no overlapping bystanders).
    pub fn update_isolated(&mut self, emb: AppearanceEmbedding) {
        if self.entries.is_empty() {
            self.entries.push(emb);
            return;
        }

        let mut best_idx = 0;
        let mut best_sim = -1.0_f32;
        for (idx, existing) in self.entries.iter().enumerate() {
            let sim = existing.cosine_similarity(&emb);
            if sim > best_sim {
                best_sim = sim;
                best_idx = idx;
            }
        }

        if best_sim >= 0.92 {
            // Very similar view: gently EMA-refresh existing anchor
            self.entries[best_idx].blend_ema(&emb, 0.08);
        } else if best_sim >= 0.65 {
            // Distinct valid angle (e.g., speaker turned sideways): append to gallery
            if self.entries.len() < self.max_entries {
                self.entries.push(emb);
            } else {
                // Keep entry 0 (original operator-clicked anchor!) permanently; rotate tail
                let replace_idx = 1 + (best_idx % (self.max_entries - 1));
                self.entries[replace_idx].blend_ema(&emb, 0.25);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_box_frame(w: u32, h: u32, rgb_a: (u8, u8, u8), rgb_b: (u8, u8, u8)) -> Vec<u8> {
        let mut buf = vec![20_u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if x < w / 2 {
                    buf[i] = rgb_a.2;
                    buf[i + 1] = rgb_a.1;
                    buf[i + 2] = rgb_a.0;
                } else {
                    buf[i] = rgb_b.2;
                    buf[i + 1] = rgb_b.1;
                    buf[i + 2] = rgb_b.0;
                }
                buf[i + 3] = 255;
            }
        }
        buf
    }

    #[test]
    fn spatial_hsv_reid_distinguishes_two_subjects_cleanly() {
        let buf = solid_box_frame(160, 120, (230, 95, 35), (45, 130, 235));
        let box_a = NormalizedRect { x: 0.05, y: 0.1, width: 0.35, height: 0.8 };
        let box_b = NormalizedRect { x: 0.55, y: 0.1, width: 0.35, height: 0.8 };

        let emb_a = extract_spatial_hsv_descriptor(&buf, 160, 120, &box_a);
        let emb_b = extract_spatial_hsv_descriptor(&buf, 160, 120, &box_b);

        let self_sim = emb_a.cosine_similarity(&emb_a);
        let cross_sim = emb_a.cosine_similarity(&emb_b);
        assert!(self_sim > 0.99);
        assert!(cross_sim < 0.70, "expected distinct clothing descriptors, got {cross_sim:.3}");
    }

    #[test]
    fn mismatched_empty_and_undersized_inputs_fail_safely() {
        let empty = AppearanceEmbedding::from_raw(vec![]);
        let dim3 = AppearanceEmbedding::from_raw(vec![1.0, 0.0, 0.0]);
        let dim4 = AppearanceEmbedding::from_raw(vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(empty.cosine_similarity(&empty), 0.0);
        assert_eq!(dim3.cosine_similarity(&dim4), 0.0);

        // Mismatched EMA blend is a safe no-op
        let mut copy = dim3.clone();
        copy.blend_ema(&dim4, 0.5);
        assert_eq!(copy.values, dim3.values);

        // Undersized frame yields zeroed 96-dim descriptor without panicking
        let bbox = NormalizedRect { x: 0.1, y: 0.1, width: 0.5, height: 0.5 };
        let emb = extract_spatial_hsv_descriptor(&[0_u8; 8], 160, 120, &bbox);
        assert_eq!(emb.values.len(), 96);
        assert!(emb.values.iter().all(|&v| v == 0.0));
    }
}
