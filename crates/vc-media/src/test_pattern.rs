use vc_core::{DetectedSubject, Keypoint2D, NormalizedRect};

/// Generates an animated synthetic studio frame (BGRA) with moving human silhouettes,
/// returning both the pixel buffer and simulated ground-truth subject detections for Phase 1 UI testing.
pub fn render_demo_frame(
    width: u32,
    height: u32,
    pattern: &str,
    tick: u64,
    locked_track_id: Option<u64>,
    bgra: &mut [u8],
) -> Vec<DetectedSubject> {
    let w = width as usize;
    let h = height as usize;
    let t = tick as f32 * 0.035;

    // Studio backdrop gradient / color bars
    for y in 0..h {
        let yf = y as f32 / h as f32;
        for x in 0..w {
            let idx = (y * w + x) * 4;
            let (r, g, b) = if pattern == "smpte" && y < h / 6 {
                let bar = (x * 7) / w.max(1);
                match bar {
                    0 => (190, 190, 190),
                    1 => (190, 190, 20),
                    2 => (20, 190, 190),
                    3 => (20, 190, 20),
                    4 => (190, 20, 190),
                    5 => (190, 20, 20),
                    _ => (20, 20, 190),
                }
            } else {
                // Dark broadcast stage backdrop with subtle warm/cool uplighting
                let xf = x as f32 / w as f32;
                let glow = ((xf - 0.5).powi(2) * -2.2).exp();
                let r = (18.0 + 28.0 * glow + 16.0 * (1.0 - yf)) as u8;
                let g = (22.0 + 36.0 * glow + 18.0 * (1.0 - yf)) as u8;
                let b = (34.0 + 62.0 * glow + 26.0 * (1.0 - yf)) as u8;
                (r, g, b)
            };
            bgra[idx] = b;
            bgra[idx + 1] = g;
            bgra[idx + 2] = r;
            bgra[idx + 3] = 255;
        }
    }

    // Stage floor horizon line
    let floor_y = (h as f32 * 0.78) as usize;
    if floor_y < h {
        for x in 0..w {
            let idx = (floor_y * w + x) * 4;
            bgra[idx] = 85;
            bgra[idx + 1] = 75;
            bgra[idx + 2] = 65;
        }
    }

    // Two moving subjects on stage so click-to-track & multi-person UI can be tested immediately
    let subj1_cx = 0.50 + 0.22 * (t * 0.75).sin();
    let subj1_cy = 0.56;
    let subj2_cx = if pattern == "ball" {
        0.50 - 0.25 * (t * 0.52 + 1.1).cos()
    } else {
        0.28 + 0.12 * (t * 0.45).cos()
    };
    let subj2_cy = 0.58;

    let specs = [
        (101_u64, subj1_cx, subj1_cy, 0.14_f32, 0.52_f32, (230_u8, 130_u8, 70_u8)),
        (102_u64, subj2_cx, subj2_cy, 0.13_f32, 0.48_f32, (90_u8, 170_u8, 235_u8)),
    ];

    let mut subjects = Vec::with_capacity(specs.len());
    for (track_id, cx, cy, bw, bh, (sr, sg, sb)) in specs {
        let bx = (cx - bw * 0.5).clamp(0.02, 0.96 - bw);
        let by = (cy - bh * 0.5).clamp(0.05, 0.96 - bh);

        // Draw torso & head silhouette
        let head_cx = (cx * w as f32) as i32;
        let head_cy = ((by + bh * 0.16) * h as f32) as i32;
        let head_r = (bh * h as f32 * 0.11).max(8.0) as i32;

        draw_filled_circle(bgra, w, h, head_cx, head_cy, head_r, (225, 192, 168));
        draw_filled_rect(
            bgra,
            w,
            h,
            ((cx - bw * 0.36) * w as f32) as i32,
            ((by + bh * 0.28) * h as f32) as i32,
            ((cx + bw * 0.36) * w as f32) as i32,
            ((by + bh * 0.95) * h as f32) as i32,
            (sr, sg, sb),
        );

        let eye_y = by + bh * 0.15;
        let shoulder_y = by + bh * 0.30;
        let hip_y = by + bh * 0.62;
        let ankle_y = by + bh * 0.96;

        subjects.push(DetectedSubject {
            track_id,
            bbox: NormalizedRect {
                x: bx,
                y: by,
                width: bw,
                height: bh,
            },
            confidence: 0.96,
            is_locked: locked_track_id == Some(track_id),
            identity_similarity: if locked_track_id == Some(track_id) {
                Some(0.94)
            } else {
                None
            },
            keypoints: vec![
                Keypoint2D { x: cx, y: eye_y + 0.015, score: 0.97 },       // nose
                Keypoint2D { x: cx - 0.015, y: eye_y, score: 0.96 },       // left eye
                Keypoint2D { x: cx + 0.015, y: eye_y, score: 0.96 },       // right eye
                Keypoint2D { x: cx - bw * 0.34, y: shoulder_y, score: 0.95 }, // left shoulder
                Keypoint2D { x: cx + bw * 0.34, y: shoulder_y, score: 0.95 }, // right shoulder
                Keypoint2D { x: cx - bw * 0.24, y: hip_y, score: 0.92 },   // left hip
                Keypoint2D { x: cx + bw * 0.24, y: hip_y, score: 0.92 },   // right hip
                Keypoint2D { x: cx - bw * 0.18, y: ankle_y, score: 0.88 }, // left ankle
                Keypoint2D { x: cx + bw * 0.18, y: ankle_y, score: 0.88 }, // right ankle
            ],
            face_sharpness: Some(0.84 + 0.08 * (t * 0.9).sin()),
        });
    }

    subjects
}

fn draw_filled_circle(
    bgra: &mut [u8],
    w: usize,
    h: usize,
    cx: i32,
    cy: i32,
    r: i32,
    (cr, cg, cb): (u8, u8, u8),
) {
    let r2 = r * r;
    let y0 = (cy - r).max(0) as usize;
    let y1 = (cy + r).min(h as i32 - 1) as usize;
    let x0 = (cx - r).max(0) as usize;
    let x1 = (cx + r).min(w as i32 - 1) as usize;
    for y in y0..=y1 {
        let dy = y as i32 - cy;
        for x in x0..=x1 {
            let dx = x as i32 - cx;
            if dx * dx + dy * dy <= r2 {
                let idx = (y * w + x) * 4;
                bgra[idx] = cb;
                bgra[idx + 1] = cg;
                bgra[idx + 2] = cr;
            }
        }
    }
}

fn draw_filled_rect(
    bgra: &mut [u8],
    w: usize,
    h: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    (cr, cg, cb): (u8, u8, u8),
) {
    let x0 = x0.max(0).min(w as i32 - 1) as usize;
    let x1 = x1.max(0).min(w as i32 - 1) as usize;
    let y0 = y0.max(0).min(h as i32 - 1) as usize;
    let y1 = y1.max(0).min(h as i32 - 1) as usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let idx = (y * w + x) * 4;
            bgra[idx] = cb;
            bgra[idx + 1] = cg;
            bgra[idx + 2] = cr;
        }
    }
}
