//! Subject-aware focus sharpness estimator using normalized Tenengrad (Sobel gradient) energy.
//! Computes sharpness strictly inside the tracked subject's face / upper-head ROI so sharp
//! stage backgrounds or LED walls never fool the auto-focus system.

use stagelock_core::{Keypoint2D, NormalizedRect};

/// Compute normalized focus sharpness (`0.0..=1.0`) inside the subject's face/head ROI.
pub fn compute_subject_face_sharpness(
    bgra: &[u8],
    width: u32,
    height: u32,
    bbox: &NormalizedRect,
    keypoints: &[Keypoint2D],
) -> f32 {
    if width < 8 || height < 8 || bgra.len() < (width * height * 4) as usize {
        return 0.0;
    }

    let roi = derive_face_roi(bbox, keypoints);
    let x0 = ((roi.x * width as f32).floor() as usize).clamp(1, width as usize - 2);
    let y0 = ((roi.y * height as f32).floor() as usize).clamp(1, height as usize - 2);
    let x1 = (((roi.x + roi.width) * width as f32).ceil() as usize).clamp(x0 + 1, width as usize - 1);
    let y1 = (((roi.y + roi.height) * height as f32).ceil() as usize).clamp(y0 + 1, height as usize - 1);

    let w = width as usize;
    let luma = |x: usize, y: usize| -> f32 {
        let i = (y * w + x) * 4;
        // BGRA -> Rec.709 luminance in [0..255]
        0.0722 * bgra[i] as f32 + 0.7152 * bgra[i + 1] as f32 + 0.2126 * bgra[i + 2] as f32
    };

    let mut grad_sum = 0.0_f64;
    let mut luma_sum = 0.0_f64;
    let mut luma_sq_sum = 0.0_f64;
    let mut count = 0_u64;

    // Stride by 2 on large ROIs to stay under 0.1ms per camera
    let step = if (x1 - x0) > 120 { 2 } else { 1 };
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let tl = luma(x - 1, y - 1);
            let tc = luma(x, y - 1);
            let tr = luma(x + 1, y - 1);
            let ml = luma(x - 1, y);
            let mc = luma(x, y);
            let mr = luma(x + 1, y);
            let bl = luma(x - 1, y + 1);
            let bc = luma(x, y + 1);
            let br = luma(x + 1, y + 1);

            let gx = -tl + tr - 2.0 * ml + 2.0 * mr - bl + br;
            let gy = -tl - 2.0 * tc - tr + bl + 2.0 * bc + br;
            grad_sum += (gx * gx + gy * gy) as f64;
            luma_sum += mc as f64;
            luma_sq_sum += (mc * mc) as f64;
            count += 1;
            x += step;
        }
        y += step;
    }

    if count == 0 {
        return 0.0;
    }

    let mean_grad = (grad_sum / count as f64).sqrt();
    let mean_luma = luma_sum / count as f64;
    let var_luma = (luma_sq_sum / count as f64 - mean_luma * mean_luma).max(16.0);
    let contrast_norm = var_luma.sqrt().max(12.0);

    // Dimensionless sharpness ratio mapped smoothly to [0.0, 1.0]
    let ratio = mean_grad / (contrast_norm * 3.8);
    (ratio as f32 / (1.0 + ratio as f32 * 0.45)).clamp(0.0, 1.0)
}

/// Derive a tight face/head ROI from COCO nose/eye keypoints when available,
/// otherwise default to the upper-center head region of the person bounding box.
pub fn derive_face_roi(bbox: &NormalizedRect, keypoints: &[Keypoint2D]) -> NormalizedRect {
    // COCO indices: 0 = nose, 1 = left_eye, 2 = right_eye
    let head_pts: Vec<&Keypoint2D> = keypoints
        .iter()
        .take(3)
        .filter(|kp| kp.score > 0.35)
        .collect();

    if !head_pts.is_empty() {
        let cx = head_pts.iter().map(|p| p.x).sum::<f32>() / head_pts.len() as f32;
        let cy = head_pts.iter().map(|p| p.y).sum::<f32>() / head_pts.len() as f32;
        let half_w = (bbox.width * 0.32).clamp(0.03, 0.20);
        let half_h = (bbox.height * 0.14).clamp(0.03, 0.22);
        return NormalizedRect {
            x: (cx - half_w).clamp(0.01, 0.94),
            y: (cy - half_h).clamp(0.01, 0.94),
            width: (half_w * 2.0).min(0.98 - (cx - half_w).clamp(0.01, 0.94)),
            height: (half_h * 2.0).min(0.98 - (cy - half_h).clamp(0.01, 0.94)),
        };
    }

    NormalizedRect {
        x: (bbox.x + bbox.width * 0.20).clamp(0.01, 0.94),
        y: (bbox.y + bbox.height * 0.03).clamp(0.01, 0.94),
        width: (bbox.width * 0.60).clamp(0.04, 0.95),
        height: (bbox.height * 0.26).clamp(0.04, 0.95),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharp_roi_scores_higher_than_blurred_roi() {
        let w = 128_u32;
        let h = 128_u32;
        let mut sharp = vec![0_u8; (w * h * 4) as usize];
        // Crisp 4px checkerboard inside the head ROI
        for y in 0..h {
            for x in 0..w {
                let val = if ((x / 4) + (y / 4)) % 2 == 0 { 230 } else { 30 };
                let i = ((y * w + x) * 4) as usize;
                sharp[i] = val;
                sharp[i + 1] = val;
                sharp[i + 2] = val;
                sharp[i + 3] = 255;
            }
        }

        // Apply heavy 5x5 box blur to simulate out-of-focus lens
        let mut blurred = sharp.clone();
        for _ in 0..4 {
            let src = blurred.clone();
            for y in 2..(h - 2) {
                for x in 2..(w - 2) {
                    let mut sum = 0_u32;
                    for dy in -2_i32..=2 {
                        for dx in -2_i32..=2 {
                            let idx = (((y as i32 + dy) as u32 * w + (x as i32 + dx) as u32) * 4) as usize;
                            sum += src[idx] as u32;
                        }
                    }
                    let avg = (sum / 25) as u8;
                    let i = ((y * w + x) * 4) as usize;
                    blurred[i] = avg;
                    blurred[i + 1] = avg;
                    blurred[i + 2] = avg;
                }
            }
        }

        let bbox = NormalizedRect { x: 0.2, y: 0.1, width: 0.6, height: 0.8 };
        let s_sharp = compute_subject_face_sharpness(&sharp, w, h, &bbox, &[]);
        let s_blur = compute_subject_face_sharpness(&blurred, w, h, &bbox, &[]);
        assert!(
            s_sharp > s_blur + 0.15,
            "expected sharp ({s_sharp:.3}) >> blurred ({s_blur:.3})"
        );
    }

    #[test]
    fn undersized_or_degenerate_frames_return_zero_without_panic() {
        let bbox = NormalizedRect { x: 0.2, y: 0.1, width: 0.5, height: 0.5 };
        assert_eq!(compute_subject_face_sharpness(&[], 128, 128, &bbox, &[]), 0.0);
        assert_eq!(compute_subject_face_sharpness(&[0; 64], 4, 4, &bbox, &[]), 0.0);
        assert_eq!(compute_subject_face_sharpness(&[0; 16], 128, 128, &bbox, &[]), 0.0);

        // Out-of-bounds bbox on valid frame still clamps safely and produces finite score
        let valid = vec![128_u8; 32 * 32 * 4];
        let oob = NormalizedRect { x: 2.0, y: 2.0, width: 5.0, height: 5.0 };
        let s = compute_subject_face_sharpness(&valid, 32, 32, &oob, &[]);
        assert!(s.is_finite() && (0.0..=1.0).contains(&s));
    }
}
