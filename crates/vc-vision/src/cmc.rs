//! Lightweight Camera Motion Compensation (CMC) for panning/tilting PTZ cameras.
//!
//! Estimates global frame translation `(dx, dy)` in normalized `[0..1]` coordinates via sparse
//! block matching on background tiles (masking out foreground subjects) so BoT-SORT Kalman tracks
//! remain rock-solid even during brisk camera pans.

use vc_core::NormalizedRect;

const GRID_W: usize = 96;
const GRID_H: usize = 54;

pub struct CameraMotionCompensator {
    prev_gray: Option<Vec<u8>>,
}

impl CameraMotionCompensator {
    pub fn new() -> Self {
        Self { prev_gray: None }
    }

    pub fn reset(&mut self) {
        self.prev_gray = None;
    }

    /// Estimate normalized frame shift `(dx, dy)` between previous and current BGRA frame.
    pub fn estimate_shift(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        foreground_boxes: &[NormalizedRect],
    ) -> (f32, f32) {
        let cur = downsample_luma(bgra, width, height);
        let Some(ref prev) = self.prev_gray else {
            self.prev_gray = Some(cur);
            return (0.0, 0.0);
        };

        let mut dxs = Vec::with_capacity(24);
        let mut dys = Vec::with_capacity(24);

        let patch = 4_i32;
        let search = 6_i32;

        // Sample 6x4 background anchors across the downsampled frame
        for gy in 0..4 {
            let cy = 9 + gy * 12;
            for gx in 0..6 {
                let cx = 10 + gx * 15;
                let nx = cx as f32 / GRID_W as f32;
                let ny = cy as f32 / GRID_H as f32;

                // Skip patches lying inside any detected person so speaker motion isn't treated as camera pan
                if foreground_boxes.iter().any(|b| {
                    nx >= b.x - 0.03
                        && nx <= b.x + b.width + 0.03
                        && ny >= b.y - 0.03
                        && ny <= b.y + b.height + 0.03
                }) {
                    continue;
                }

                // Skip flat low-texture patches (blank walls/ceilings)
                if patch_variance(prev, cx, cy, patch) < 25.0 {
                    continue;
                }

                let mut best_sad = u32::MAX;
                let mut best_dx = 0_i32;
                let mut best_dy = 0_i32;

                for dy in -search..=search {
                    for dx in -search..=search {
                        let sad = patch_sad(prev, &cur, cx, cy, cx + dx, cy + dy, patch);
                        if sad < best_sad {
                            best_sad = sad;
                            best_dx = dx;
                            best_dy = dy;
                        }
                    }
                }

                dxs.push(best_dx as f32 / GRID_W as f32);
                dys.push(best_dy as f32 / GRID_H as f32);
            }
        }

        self.prev_gray = Some(cur);

        if dxs.len() < 3 {
            return (0.0, 0.0);
        }

        dxs.sort_by(|a, b| a.total_cmp(b));
        dys.sort_by(|a, b| a.total_cmp(b));
        let mid = dxs.len() / 2;
        (dxs[mid], dys[mid])
    }
}

impl Default for CameraMotionCompensator {
    fn default() -> Self {
        Self::new()
    }
}

fn downsample_luma(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0_u8; GRID_W * GRID_H];
    if width == 0 || height == 0 || bgra.len() < (width * height * 4) as usize {
        return out;
    }
    let w = width as usize;
    let h = height as usize;
    for gy in 0..GRID_H {
        let sy = (gy * h) / GRID_H;
        for gx in 0..GRID_W {
            let sx = (gx * w) / GRID_W;
            let i = (sy * w + sx) * 4;
            let y = 0.0722 * bgra[i] as f32 + 0.7152 * bgra[i + 1] as f32 + 0.2126 * bgra[i + 2] as f32;
            out[gy * GRID_W + gx] = y as u8;
        }
    }
    out
}

fn patch_variance(img: &[u8], cx: i32, cy: i32, half: i32) -> f32 {
    let mut sum = 0_f32;
    let mut sq = 0_f32;
    let mut n = 0_f32;
    for dy in -half..=half {
        let y = (cy + dy).clamp(0, GRID_H as i32 - 1) as usize;
        for dx in -half..=half {
            let x = (cx + dx).clamp(0, GRID_W as i32 - 1) as usize;
            let v = img[y * GRID_W + x] as f32;
            sum += v;
            sq += v * v;
            n += 1.0;
        }
    }
    let mean = sum / n;
    (sq / n) - mean * mean
}

fn patch_sad(prev: &[u8], cur: &[u8], x0: i32, y0: i32, x1: i32, y1: i32, half: i32) -> u32 {
    let mut sad = 0_u32;
    for dy in -half..=half {
        let py = (y0 + dy).clamp(0, GRID_H as i32 - 1) as usize;
        let cy = (y1 + dy).clamp(0, GRID_H as i32 - 1) as usize;
        for dx in -half..=half {
            let px = (x0 + dx).clamp(0, GRID_W as i32 - 1) as usize;
            let cx = (x1 + dx).clamp(0, GRID_W as i32 - 1) as usize;
            let a = prev[py * GRID_W + px];
            let b = cur[cy * GRID_W + cx];
            sad += a.abs_diff(b) as u32;
        }
    }
    sad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_horizontal_camera_pan_shift() {
        let w = 192_u32;
        let h = 108_u32;
        let make_frame = |shift_x: i32| -> Vec<u8> {
            let mut buf = vec![0_u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let sx = (x as i32 - shift_x).rem_euclid(w as i32) as u32;
                    let v = (((sx / 12) ^ (y / 12)) * 37 % 255) as u8;
                    let i = ((y * w + x) * 4) as usize;
                    buf[i] = v;
                    buf[i + 1] = v;
                    buf[i + 2] = v;
                    buf[i + 3] = 255;
                }
            }
            buf
        };

        let mut cmc = CameraMotionCompensator::new();
        let f1 = make_frame(0);
        let f2 = make_frame(8); // +8px at 192px wide = +4/96 ≈ +0.0416 normalized

        assert_eq!(cmc.estimate_shift(&f1, w, h, &[]), (0.0, 0.0));
        let (dx, dy) = cmc.estimate_shift(&f2, w, h, &[]);
        assert!((dx - (8.0 / 192.0)).abs() < 0.02, "expected dx ~0.042, got {dx:.4}");
        assert!(dy.abs() < 0.02, "expected dy ~0.0, got {dy:.4}");
    }

    #[test]
    fn undersized_and_flat_low_texture_frames_return_zero_shift() {
        let mut cmc = CameraMotionCompensator::new();
        assert_eq!(cmc.estimate_shift(&[], 192, 108, &[]), (0.0, 0.0));
        assert_eq!(cmc.estimate_shift(&[128_u8; 16], 192, 108, &[]), (0.0, 0.0));

        // Perfectly uniform frames have zero variance (< 25) and must return (0.0, 0.0)
        let flat = vec![100_u8; 192 * 108 * 4];
        assert_eq!(cmc.estimate_shift(&flat, 192, 108, &[]), (0.0, 0.0));
        assert_eq!(cmc.estimate_shift(&flat, 192, 108, &[]), (0.0, 0.0));
    }
}
