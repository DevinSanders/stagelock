use vc_core::ColorAdjustments;

/// Fast in-place BGRA color adjustment & real-time Unsharp-Mask sharpness pass
/// (White Balance Temp/Tint, Exposure, Brightness, Contrast, Saturation, Sharpness).
pub fn apply_color_adjustments_bgra(buf: &mut [u8], width: u32, height: u32, adj: &ColorAdjustments) {
    let color_is_identity = adj.white_balance_temp.abs() < 1e-4
        && adj.white_balance_tint.abs() < 1e-4
        && adj.exposure.abs() < 1e-4
        && (adj.contrast - 1.0).abs() < 1e-4
        && (adj.saturation - 1.0).abs() < 1e-4
        && adj.brightness.abs() < 1e-4;

    if !color_is_identity {
        let exp_gain = 2.0_f32.powf(adj.exposure.clamp(-2.0, 2.0));
        let r_wb = (1.0 + adj.white_balance_temp * 0.25 + adj.white_balance_tint * 0.10).clamp(0.4, 2.0);
        let g_wb = (1.0 - adj.white_balance_tint * 0.15).clamp(0.4, 2.0);
        let b_wb = (1.0 - adj.white_balance_temp * 0.25 + adj.white_balance_tint * 0.10).clamp(0.4, 2.0);

        let contrast = adj.contrast.clamp(0.0, 2.0);
        let saturation = adj.saturation.clamp(0.0, 2.0);
        let brightness = adj.brightness.clamp(-1.0, 1.0);

        for px in buf.chunks_exact_mut(4) {
            let mut b = (px[0] as f32 / 255.0) * exp_gain * b_wb;
            let mut g = (px[1] as f32 / 255.0) * exp_gain * g_wb;
            let mut r = (px[2] as f32 / 255.0) * exp_gain * r_wb;

            r = (r - 0.5) * contrast + 0.5 + brightness;
            g = (g - 0.5) * contrast + 0.5 + brightness;
            b = (b - 0.5) * contrast + 0.5 + brightness;

            if (saturation - 1.0).abs() > 1e-4 {
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                r = luma + (r - luma) * saturation;
                g = luma + (g - luma) * saturation;
                b = luma + (b - luma) * saturation;
            }

            px[0] = (b.clamp(0.0, 1.0) * 255.0) as u8;
            px[1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
            px[2] = (r.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }

    // Real-time 5-tap Laplacian Unsharp Mask for sharpness > 0
    let amount = adj.sharpness.clamp(0.0, 2.0);
    if amount > 0.02 && width >= 3 && height >= 3 {
        let w = width as usize;
        let h = height as usize;
        if buf.len() >= w * h * 4 {
            apply_unsharp_mask_bgra(buf, w, h, amount * 0.45);
        }
    }
}

fn apply_unsharp_mask_bgra(buf: &mut [u8], w: usize, h: usize, strength: f32) {
    let src = buf.to_vec();
    let row = w * 4;
    for y in 1..(h - 1) {
        let y_off = y * row;
        for x in 1..(w - 1) {
            let i = y_off + x * 4;
            let up = i - row;
            let dn = i + row;
            let lt = i - 4;
            let rt = i + 4;
            for c in 0..3 {
                let center = src[i + c] as f32;
                let avg = (src[up + c] as f32
                    + src[dn + c] as f32
                    + src[lt + c] as f32
                    + src[rt + c] as f32)
                    * 0.25;
                let sharp = center + (center - avg) * strength;
                buf[i + c] = sharp.clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_color_adjustment_is_noop() {
        let mut frame = vec![10, 128, 240, 255, 50, 60, 70, 255];
        let orig = frame.clone();
        apply_color_adjustments_bgra(&mut frame, 2, 1, &ColorAdjustments::default());
        assert_eq!(frame, orig);
    }

    #[test]
    fn exposure_and_warmth_adjust_channels() {
        let mut frame = vec![100, 100, 100, 255];
        let adj = ColorAdjustments {
            exposure: 1.0,
            white_balance_temp: 0.5,
            ..Default::default()
        };
        apply_color_adjustments_bgra(&mut frame, 1, 1, &adj);
        assert!(frame[2] > frame[0], "warm WB should boost R above B");
        assert!(frame[1] > 150, "+1 stop exposure should brighten pixel");
    }

    #[test]
    fn unsharp_mask_enhances_edge_contrast() {
        // 3x3 frame with bright center pixel surrounded by dark pixels
        let mut frame = vec![40, 40, 40, 255].repeat(9);
        let center = 4 * 4;
        frame[center] = 160;
        frame[center + 1] = 160;
        frame[center + 2] = 160;

        let adj = ColorAdjustments {
            sharpness: 1.0,
            ..Default::default()
        };
        apply_color_adjustments_bgra(&mut frame, 3, 3, &adj);
        assert!(frame[center] > 160, "unsharp mask should boost bright peak");
    }
}
