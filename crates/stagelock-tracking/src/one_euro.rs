//! Adaptive One-Euro low-pass filter (Casiez et al., CHI 2012).
//! Smooths high-frequency detector/keypoint micro-jitter heavily when the subject is still,
//! while automatically lowering latency during brisk walking movements.

use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct OneEuroFilter {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    prev_x: Option<f32>,
    prev_dx: f32,
}

impl OneEuroFilter {
    pub fn new(min_cutoff: f32, beta: f32, d_cutoff: f32) -> Self {
        Self {
            min_cutoff: min_cutoff.max(0.01),
            beta: beta.max(0.0),
            d_cutoff: d_cutoff.max(0.01),
            prev_x: None,
            prev_dx: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.prev_x = None;
        self.prev_dx = 0.0;
    }

    pub fn filter(&mut self, value: f32, dt: f32) -> f32 {
        let dt = dt.max(1e-3);
        let Some(prev_x) = self.prev_x else {
            self.prev_x = Some(value);
            self.prev_dx = 0.0;
            return value;
        };

        let raw_dx = (value - prev_x) / dt;
        let alpha_d = smoothing_factor(dt, self.d_cutoff);
        let dx_hat = exponential_smooth(alpha_d, raw_dx, self.prev_dx);
        self.prev_dx = dx_hat;

        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let alpha = smoothing_factor(dt, cutoff);
        let x_hat = exponential_smooth(alpha, value, prev_x);
        self.prev_x = Some(x_hat);
        x_hat
    }

    pub fn velocity(&self) -> f32 {
        self.prev_dx
    }
}

fn smoothing_factor(dt: f32, cutoff: f32) -> f32 {
    let r = 2.0 * PI * cutoff * dt;
    r / (r + 1.0)
}

fn exponential_smooth(alpha: f32, x: f32, prev: f32) -> f32 {
    alpha * x + (1.0 - alpha) * prev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_stationary_jitter_while_tracking_motion() {
        let mut f = OneEuroFilter::new(0.8, 3.5, 1.0);
        let dt = 1.0 / 30.0;
        let mut max_dev = 0.0_f32;
        // Inject ±0.01 alternating pixel jitter around 0.50
        for i in 0..60 {
            let jitter = if i % 2 == 0 { 0.01 } else { -0.01 };
            let out = f.filter(0.50 + jitter, dt);
            if i > 15 {
                max_dev = max_dev.max((out - 0.50).abs());
            }
        }
        assert!(max_dev < 0.0025, "expected heavy jitter suppression (<0.0025), got {max_dev:.5}");

        // Now move rapidly to 0.80 and verify swift convergence
        let mut out = 0.50;
        for _ in 0..20 {
            out = f.filter(0.80, dt);
        }
        assert!(out > 0.77, "expected responsive tracking to 0.80, got {out:.4}");
    }

    #[test]
    fn zero_or_negative_dt_clamps_without_nan_or_inf() {
        let mut f = OneEuroFilter::new(0.0, -5.0, 0.0);
        let v0 = f.filter(0.5, 0.0);
        let v1 = f.filter(0.7, 0.0);
        let v2 = f.filter(0.2, -1.0);
        assert!(v0.is_finite() && v1.is_finite() && v2.is_finite());
        assert!(f.velocity().is_finite());
    }
}
