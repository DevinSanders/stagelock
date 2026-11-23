//! Unified per-camera closed-loop tracking + auto-framing + auto-focus controller.
//! Each PTZ camera runs its own independent `CameraTrackingController` instance so multiple
//! cameras track different subjects simultaneously without interference.

use vc_core::{
    DetectedSubject, FocusMonitorState, FramingConfig, NormalizedRect, PtzVelocity, TallyState,
    TrackingState,
};
use vc_ptz::FocusCommand;

use crate::autofocus::AutoFocusController;
use crate::framing::FramingSolver;
pub use crate::motion::CameraCalibration;
use crate::motion::SmoothMotionController;

#[derive(Debug, Clone)]
pub struct TrackingTickOutput {
    pub ptz_velocity: PtzVelocity,
    pub target_anchor: [f32; 2],
    pub deadzone: NormalizedRect,
    pub focus_state: FocusMonitorState,
    pub focus_command: Option<FocusCommand>,
    pub should_send_ptz: bool,
}

pub struct CameraTrackingController {
    framing: FramingSolver,
    motion: SmoothMotionController,
    autofocus: AutoFocusController,
    manual_override_hold_s: f32,
    was_moving: bool,
}

impl CameraTrackingController {
    pub fn new(calibration: CameraCalibration) -> Self {
        Self {
            framing: FramingSolver::new(),
            motion: SmoothMotionController::new(calibration),
            autofocus: AutoFocusController::new(),
            manual_override_hold_s: 0.0,
            was_moving: false,
        }
    }

    pub fn set_calibration(&mut self, cal: CameraCalibration) {
        self.motion.set_calibration(cal);
    }

    pub fn calibration(&self) -> &CameraCalibration {
        self.motion.calibration()
    }

    /// Notify controller that the operator manually moved the joystick or recalled a preset,
    /// pausing automated commands briefly so manual control always takes priority.
    pub fn notify_manual_override(&mut self, hold_seconds: f32) {
        self.manual_override_hold_s = hold_seconds.max(0.5);
        self.motion.reset();
        self.was_moving = false;
    }

    /// Execute one closed-loop control tick (`dt` typically ~0.033s at 30 Hz).
    pub fn tick(
        &mut self,
        config: &FramingConfig,
        tracking_state: TrackingState,
        locked_subject: Option<&DetectedSubject>,
        face_sharpness: f32,
        tally: TallyState,
        dt: f32,
    ) -> TrackingTickOutput {
        let dt = dt.clamp(0.005, 0.2);

        if self.manual_override_hold_s > 0.0 {
            self.manual_override_hold_s = (self.manual_override_hold_s - dt).max(0.0);
            let anchor = self.framing.current_anchor(config.preset);
            return TrackingTickOutput {
                ptz_velocity: PtzVelocity::default(),
                target_anchor: anchor,
                deadzone: make_deadzone(anchor, config),
                focus_state: FocusMonitorState::MotionGated,
                focus_command: None,
                should_send_ptz: false,
            };
        }

        if !config.auto_track_enabled {
            let vel = if self.was_moving {
                self.motion.decelerate_to_stop(dt)
            } else {
                PtzVelocity::default()
            };
            let is_moving = is_nonzero(vel);
            let should_send = is_moving || self.was_moving;
            self.was_moving = is_moving;
            let anchor = self.framing.current_anchor(config.preset);
            return TrackingTickOutput {
                ptz_velocity: vel,
                target_anchor: anchor,
                deadzone: make_deadzone(anchor, config),
                focus_state: FocusMonitorState::Sharp,
                focus_command: None,
                should_send_ptz: should_send,
            };
        }

        let (vel, anchor, deadzone, subject_locked) = match (tracking_state, locked_subject) {
            (TrackingState::Locked, Some(subject)) => {
                let svx = self.motion.subject_vx();
                let ferr = self.framing.compute(subject, config, svx, dt);
                let vel = self.motion.update_locked(&ferr, config, tally, dt);
                (vel, [ferr.target_x, ferr.target_y], ferr.deadzone, true)
            }
            // OccludedHold or Searching: smoothly feather to a stop without swinging wildly
            _ => {
                let vel = self.motion.decelerate_to_stop(dt);
                let anchor = self.framing.current_anchor(config.preset);
                (vel, anchor, make_deadzone(anchor, config), false)
            }
        };

        let is_moving = is_nonzero(vel);
        let should_send_ptz = is_moving || self.was_moving;
        self.was_moving = is_moving;

        let af = self.autofocus.update(
            config.auto_focus_enabled,
            config.focus_strategy,
            face_sharpness,
            subject_locked,
            vel,
            self.motion.subject_vx(),
            dt,
        );

        TrackingTickOutput {
            ptz_velocity: vel,
            target_anchor: anchor,
            deadzone,
            focus_state: af.state,
            focus_command: af.command,
            should_send_ptz,
        }
    }
}

fn is_nonzero(v: PtzVelocity) -> bool {
    v.pan.abs() > 1e-3 || v.tilt.abs() > 1e-3 || v.zoom.abs() > 1e-3
}

fn make_deadzone(anchor: [f32; 2], cfg: &FramingConfig) -> NormalizedRect {
    NormalizedRect {
        x: (anchor[0] - cfg.deadzone_x).clamp(0.0, 1.0),
        y: (anchor[1] - cfg.deadzone_y).clamp(0.0, 1.0),
        width: (cfg.deadzone_x * 2.0).min(1.0),
        height: (cfg.deadzone_y * 2.0).min(1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use vc_core::{FramingPreset, Keypoint2D, RuleOfThirds};

    /// Closed-loop virtual PTZ camera simulator with 165ms pipeline latency (5 frames @ 30fps).
    struct SimCamera {
        cam_pan: f32,
        cam_tilt: f32,
        cam_zoom: f32, // optical scale multiplier (1.0 = wide, 2.5 = close-up)
        cmd_queue: VecDeque<PtzVelocity>,
    }

    impl SimCamera {
        fn new() -> Self {
            let mut q = VecDeque::new();
            for _ in 0..5 {
                q.push_back(PtzVelocity::default());
            }
            Self {
                cam_pan: 0.0,
                cam_tilt: 0.0,
                cam_zoom: 1.0,
                cmd_queue: q,
            }
        }

        fn step(&mut self, next_cmd: PtzVelocity, dt: f32) {
            self.cmd_queue.push_back(next_cmd);
            let active = self.cmd_queue.pop_front().unwrap_or_default();
            self.cam_pan += active.pan * 0.45 * dt;
            self.cam_tilt += active.tilt * 0.35 * dt;
            self.cam_zoom = (self.cam_zoom + active.zoom * 0.55 * dt).clamp(0.6, 3.0);
        }

        fn observe_speaker(&self, world_x: f32, world_y: f32, world_h: f32) -> DetectedSubject {
            let screen_x = 0.5 + (world_x - self.cam_pan) * self.cam_zoom;
            let screen_eye_y = 0.30 - (world_y - self.cam_tilt) * self.cam_zoom;
            let screen_h = world_h * self.cam_zoom;
            let screen_w = screen_h * 0.32;

            DetectedSubject {
                track_id: 1,
                bbox: NormalizedRect {
                    x: (screen_x - screen_w * 0.5).clamp(0.0, 1.0),
                    y: (screen_eye_y - screen_h * 0.15).clamp(0.0, 1.0),
                    width: screen_w.clamp(0.05, 0.9),
                    height: screen_h.clamp(0.08, 0.95),
                },
                confidence: 0.96,
                is_locked: true,
                identity_similarity: Some(0.95),
                keypoints: vec![
                    Keypoint2D { x: screen_x, y: screen_eye_y + 0.01, score: 0.96 },
                    Keypoint2D { x: screen_x - 0.01, y: screen_eye_y, score: 0.96 },
                    Keypoint2D { x: screen_x + 0.01, y: screen_eye_y, score: 0.96 },
                    Keypoint2D { x: screen_x - screen_w * 0.45, y: screen_eye_y + screen_h * 0.18, score: 0.95 },
                    Keypoint2D { x: screen_x + screen_w * 0.45, y: screen_eye_y + screen_h * 0.18, score: 0.95 },
                    Keypoint2D { x: screen_x - screen_w * 0.32, y: screen_eye_y + screen_h * 0.50, score: 0.93 },
                    Keypoint2D { x: screen_x + screen_w * 0.32, y: screen_eye_y + screen_h * 0.50, score: 0.93 },
                    Keypoint2D { x: screen_x - screen_w * 0.25, y: screen_eye_y + screen_h * 0.85, score: 0.90 },
                    Keypoint2D { x: screen_x + screen_w * 0.25, y: screen_eye_y + screen_h * 0.85, score: 0.90 },
                ],
                face_sharpness: Some(0.88),
            }
        }
    }

    #[test]
    fn closed_loop_step_response_converges_cleanly_without_overshoot_or_jitter() {
        let mut cam = SimCamera::new();
        let mut ctrl = CameraTrackingController::new(CameraCalibration::default());
        let mut cfg = FramingConfig::default();
        cfg.auto_track_enabled = true;
        cfg.preset = FramingPreset::WaistUp;
        cfg.rule_of_thirds = RuleOfThirds::LeftThird;

        let dt = 1.0 / 30.0;
        let speaker_world_x = 0.28; // stepped to the right of stage
        let mut max_overshoot_left = 0.0_f32;

        for frame in 0..180 {
            let subj = cam.observe_speaker(speaker_world_x, 0.0, 0.48);
            let out = ctrl.tick(&cfg, TrackingState::Locked, Some(&subj), 0.88, TallyState::Off, dt);
            cam.step(out.ptz_velocity, dt);

            if frame > 40 {
                let err_x = subj.keypoints[0].x - out.target_anchor[0];
                if err_x < 0.0 {
                    max_overshoot_left = max_overshoot_left.max(-err_x);
                }
            }
        }

        let final_subj = cam.observe_speaker(speaker_world_x, 0.0, 0.48);
        let final_out = ctrl.tick(&cfg, TrackingState::Locked, Some(&final_subj), 0.88, TallyState::Off, dt);
        let final_err_x = (final_subj.keypoints[0].x - 0.333).abs();

        assert!(final_err_x < 0.06, "expected clean convergence to Left Third (~0.333), err={final_err_x:.3}");
        assert!(max_overshoot_left < 0.09, "must not wildly overshoot target (overshoot={max_overshoot_left:.3})");
        assert_eq!(final_out.ptz_velocity.pan, 0.0, "must settle cleanly to 0.0 pan with zero steady-state jitter");
    }

    #[test]
    fn three_independent_ptz_cameras_track_different_subjects_simultaneously() {
        let presets = [
            (FramingPreset::CloseUp, RuleOfThirds::LeftThird, 0.25_f32),
            (FramingPreset::WaistUp, RuleOfThirds::Center, -0.22_f32),
            (FramingPreset::FullHeight, RuleOfThirds::RightThird, 0.18_f32),
        ];

        let mut cams = [SimCamera::new(), SimCamera::new(), SimCamera::new()];
        let mut ctrls = [
            CameraTrackingController::new(CameraCalibration::default()),
            CameraTrackingController::new(CameraCalibration::default()),
            CameraTrackingController::new(CameraCalibration::default()),
        ];

        let dt = 1.0 / 30.0;
        for _ in 0..180 {
            for i in 0..3 {
                let mut cfg = FramingConfig::default();
                cfg.auto_track_enabled = true;
                cfg.preset = presets[i].0;
                cfg.rule_of_thirds = presets[i].1;
                let subj = cams[i].observe_speaker(presets[i].2, 0.0, 0.45);
                let out = ctrls[i].tick(&cfg, TrackingState::Locked, Some(&subj), 0.85, TallyState::Off, dt);
                cams[i].step(out.ptz_velocity, dt);
            }
        }

        // Camera 0 (Close-Up) should zoom in tighter than Camera 2 (Full Height)
        assert!(
            cams[0].cam_zoom > cams[2].cam_zoom,
            "Close-Up cam zoom ({:.2}) should exceed Full-Height cam zoom ({:.2})",
            cams[0].cam_zoom,
            cams[2].cam_zoom
        );
        // Camera 0 panned right (+), Camera 1 panned left (-)
        assert!(cams[0].cam_pan > 0.10);
        assert!(cams[1].cam_pan < -0.10);
    }
}
