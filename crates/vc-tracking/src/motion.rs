//! 5-Stage Broadcast Smooth PTZ Motion Controller:
//! 1. Adaptive One-Euro Filter on framing anchor & scale
//! 2. Per-axis Nested Dead-Zone with Hysteresis (engage outside outer box, settle inside inner box)
//! 3. Zoom-Compensated PD + Velocity Feed-Forward + optional vMix Tally-Aware Damping
//! 4. S-Curve Acceleration & Jerk Limiting (human fluid-head feathering)
//! 5. Decoupled Slow Zoom + Per-Camera Speed Calibration Mapping

pub use vc_core::CameraCalibration;
use vc_core::{FramingConfig, PtzVelocity, TallyState};

use crate::framing::FramingError;
use crate::one_euro::OneEuroFilter;

#[derive(Debug, Clone)]
struct AxisState {
    engaged: bool,
    velocity: f32,
    accel: f32,
    prev_err: f32,
}

impl Default for AxisState {
    fn default() -> Self {
        Self {
            engaged: false,
            velocity: 0.0,
            accel: 0.0,
            prev_err: 0.0,
        }
    }
}

pub struct SmoothMotionController {
    filter_x: OneEuroFilter,
    filter_y: OneEuroFilter,
    filter_z: OneEuroFilter,
    pan: AxisState,
    tilt: AxisState,
    zoom: AxisState,
    /// Estimated normalized optical zoom level `[0.0 wide .. 1.0 full tele]`
    estimated_zoom_level: f32,
    calibration: CameraCalibration,
}

impl SmoothMotionController {
    pub fn new(calibration: CameraCalibration) -> Self {
        Self {
            filter_x: OneEuroFilter::new(1.1, 4.0, 1.0),
            filter_y: OneEuroFilter::new(1.0, 3.0, 1.0),
            filter_z: OneEuroFilter::new(0.5, 1.5, 1.0),
            pan: AxisState::default(),
            tilt: AxisState::default(),
            zoom: AxisState::default(),
            estimated_zoom_level: 0.25,
            calibration,
        }
    }

    pub fn set_calibration(&mut self, cal: CameraCalibration) {
        self.calibration = cal;
    }

    pub fn calibration(&self) -> &CameraCalibration {
        &self.calibration
    }

    pub fn reset(&mut self) {
        self.filter_x.reset();
        self.filter_y.reset();
        self.filter_z.reset();
        self.pan = AxisState::default();
        self.tilt = AxisState::default();
        self.zoom = AxisState::default();
    }

    pub fn estimated_zoom_level(&self) -> f32 {
        self.estimated_zoom_level
    }

    pub fn set_estimated_zoom_level(&mut self, level: f32) {
        self.estimated_zoom_level = level.clamp(0.0, 1.0);
    }

    /// Subject horizontal velocity estimated from the One-Euro filter (frame-widths / sec).
    pub fn subject_vx(&self) -> f32 {
        self.filter_x.velocity()
    }

    /// Compute smooth, jerk-limited PTZ velocity commands when subject is actively locked.
    pub fn update_locked(
        &mut self,
        framing: &FramingError,
        config: &FramingConfig,
        tally: TallyState,
        dt: f32,
    ) -> PtzVelocity {
        let dt = dt.clamp(0.005, 0.2);

        // Stage 1: One-Euro adaptive smoothing on framing errors
        let ex = self.filter_x.filter(framing.err_x, dt);
        let ey = self.filter_y.filter(framing.err_y, dt);
        let ez = self.filter_z.filter(framing.err_zoom, dt);

        // Zoom-compensated angular gain (telephoto shots require finer speeds!)
        let zoom_comp = 1.0 / (1.0 + 2.2 * self.estimated_zoom_level);
        // Tally-aware broadcast damping: gentler on-air movements while Live on Program
        let tally_scale = if tally == TallyState::Program && config.tally_damping_enabled {
            0.62
        } else {
            1.0
        };
        let speed_cap = config.max_speed.clamp(0.1, 1.0) * tally_scale;

        // Velocity feed-forward ONLY assists when error is actively growing (subject walking away),
        // preventing braking stalls as the camera smoothly closes in on the inner settle zone.
        let latency_s = (self.calibration.latency_ms / 1000.0).clamp(0.05, 0.50);
        let vx = self.filter_x.velocity();
        let vy = self.filter_y.velocity();
        let ff_pan = if vx * ex > 0.0 {
            vx * (0.18 + latency_s * 0.35)
        } else {
            0.0
        };
        let ff_tilt = if vy * ey > 0.0 {
            vy * (0.14 + latency_s * 0.25)
        } else {
            0.0
        };

        let pan_cmd = step_axis(
            &mut self.pan,
            ex,
            ff_pan,
            config.deadzone_x.clamp(0.02, 0.35),
            2.10 * zoom_comp * self.calibration.pan_scale,
            0.09 * zoom_comp,
            speed_cap,
            2.6 * tally_scale,
            12.0 * tally_scale,
            self.calibration.min_effective_speed,
            dt,
        );

        let tilt_cmd = step_axis(
            &mut self.tilt,
            ey,
            ff_tilt,
            config.deadzone_y.clamp(0.02, 0.35),
            1.75 * zoom_comp * self.calibration.tilt_scale,
            0.08 * zoom_comp,
            speed_cap * 0.80,
            2.1 * tally_scale,
            9.5 * tally_scale,
            self.calibration.min_effective_speed,
            dt,
        );

        // Decoupled slow optical zoom with wider dead-zone to eliminate "zoom breathing"
        let zoom_cmd = step_axis(
            &mut self.zoom,
            ez,
            0.0,
            config.deadzone_zoom.clamp(0.05, 0.45),
            0.55 * self.calibration.zoom_scale,
            0.06,
            (speed_cap * 0.42).min(0.35),
            0.85 * tally_scale,
            3.5 * tally_scale,
            0.04,
            dt,
        );

        // Integrate estimated zoom position for ongoing zoom-compensated pan/tilt gain
        self.estimated_zoom_level = (self.estimated_zoom_level + zoom_cmd * dt * 0.12).clamp(0.0, 1.0);

        PtzVelocity {
            pan: pan_cmd,
            tilt: tilt_cmd,
            zoom: zoom_cmd,
        }
    }

    /// Smoothly feather PTZ motion to 0.0 during OccludedHold, Searching, or Tracking Pause.
    pub fn decelerate_to_stop(&mut self, dt: f32) -> PtzVelocity {
        let dt = dt.clamp(0.005, 0.2);
        self.pan.engaged = false;
        self.tilt.engaged = false;
        self.zoom.engaged = false;

        let p = s_curve_ramp(&mut self.pan, 0.0, 2.4, 12.0, dt);
        let t = s_curve_ramp(&mut self.tilt, 0.0, 2.0, 10.0, dt);
        let z = s_curve_ramp(&mut self.zoom, 0.0, 1.2, 6.0, dt);

        PtzVelocity {
            pan: if p.abs() < 0.015 { 0.0 } else { p },
            tilt: if t.abs() < 0.015 { 0.0 } else { t },
            zoom: if z.abs() < 0.015 { 0.0 } else { z },
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn step_axis(
    axis: &mut AxisState,
    err: f32,
    feed_forward: f32,
    outer_dz: f32,
    kp: f32,
    kd: f32,
    max_vel: f32,
    max_accel: f32,
    max_jerk: f32,
    min_effective_speed: f32,
    dt: f32,
) -> f32 {
    // Stage 2: Nested Dead-Zone Hysteresis
    let inner_settle = outer_dz * 0.35;
    if axis.engaged {
        if err.abs() <= inner_settle {
            axis.engaged = false;
        }
    } else if err.abs() > outer_dz {
        axis.engaged = true;
    }

    let derr = (err - axis.prev_err) / dt;
    axis.prev_err = err;

    let desired_vel = if !axis.engaged {
        0.0
    } else {
        // Gentle proportional drive right down to inner_settle with derivative damping
        let shaped = (err.abs() * 0.55 + (err.abs() - inner_settle).max(0.0) * 0.45) * err.signum();
        let raw = shaped * kp + derr * kd + feed_forward;
        raw.clamp(-max_vel, max_vel)
    };

    // Stage 4: S-Curve Acceleration & Jerk Limiting
    let v = s_curve_ramp(axis, desired_vel, max_accel, max_jerk, dt);

    // Motor deadband guard: cleanly zero out tiny residual commands when settling
    if !axis.engaged && v.abs() < min_effective_speed {
        axis.velocity = 0.0;
        axis.accel = 0.0;
        return 0.0;
    }

    v.clamp(-1.0, 1.0)
}

fn s_curve_ramp(axis: &mut AxisState, target_vel: f32, max_accel: f32, max_jerk: f32, dt: f32) -> f32 {
    let desired_accel = ((target_vel - axis.velocity) / dt).clamp(-max_accel, max_accel);
    let da = (desired_accel - axis.accel).clamp(-max_jerk * dt, max_jerk * dt);
    axis.accel += da;
    axis.velocity += axis.accel * dt;

    // Prevent overshoot when ramping to zero
    if target_vel.abs() < 1e-4 && axis.velocity.abs() < 0.01 {
        axis.velocity = 0.0;
        axis.accel = 0.0;
    }
    axis.velocity
}

#[cfg(test)]
mod tests {
    use super::*;
    use vc_core::NormalizedRect;

    fn err(ex: f32, ey: f32, ez: f32) -> FramingError {
        FramingError {
            subject_x: 0.5 + ex,
            subject_y: 0.3 - ey,
            subject_scale: 0.5,
            target_x: 0.5,
            target_y: 0.3,
            target_scale: 0.5,
            err_x: ex,
            err_y: ey,
            err_zoom: ez,
            deadzone: NormalizedRect { x: 0.42, y: 0.24, width: 0.16, height: 0.12 },
        }
    }

    #[test]
    fn nested_deadzone_stays_strictly_zero_on_micro_movements() {
        let mut ctrl = SmoothMotionController::new(CameraCalibration::default());
        let cfg = FramingConfig::default(); // deadzone_x = 0.08

        for i in 0..45 {
            let sway = if i % 2 == 0 { 0.05 } else { -0.05 }; // inside ±0.08 quiet zone
            let cmd = ctrl.update_locked(&err(sway, 0.03, 0.06), &cfg, TallyState::Off, 0.033);
            assert_eq!(cmd.pan, 0.0, "micro sway inside deadzone must emit 0.0 pan");
            assert_eq!(cmd.tilt, 0.0, "micro sway inside deadzone must emit 0.0 tilt");
            assert_eq!(cmd.zoom, 0.0, "small scale variance must not breathe zoom");
        }
    }

    #[test]
    fn tally_program_dampens_maximum_velocity() {
        let mut off_air = SmoothMotionController::new(CameraCalibration::default());
        let mut on_air = SmoothMotionController::new(CameraCalibration::default());
        let cfg = FramingConfig::default();

        let mut v_off = 0.0_f32;
        let mut v_on = 0.0_f32;
        for _ in 0..40 {
            v_off = v_off.max(off_air.update_locked(&err(0.45, 0.0, 0.0), &cfg, TallyState::Off, 0.033).pan);
            v_on = v_on.max(on_air.update_locked(&err(0.45, 0.0, 0.0), &cfg, TallyState::Program, 0.033).pan);
        }
        assert!(v_off > v_on * 1.25, "Program tally should dampen peak pan speed ({v_on:.3} vs {v_off:.3})");
    }
}
