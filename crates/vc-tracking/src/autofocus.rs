//! Subject-Aware 2-Tier Motion-Gated Auto-Focus Controller.
//!
//! Consumes Face-ROI Tenengrad sharpness from `vc-vision` and gates triggers against both PTZ
//! angular velocity and subject motion so motion blur is never mistaken for optical defocus.
//!
//! - Tier 1 (`AutoFocusStrategy::OnePushAf`): Universal One-Push Auto-Focus with rolling baseline & cooldown.
//! - Tier 2 (`AutoFocusStrategy::ActiveHillClimb`): Bounded manual-focus contrast maximization strictly
//!   on the locked subject's face ROI (ideal when stage LED walls trick built-in camera AF).

use vc_core::{AutoFocusStrategy, FocusMonitorState, PtzVelocity};
use vc_ptz::FocusCommand;

#[derive(Debug, Clone)]
pub struct AutoFocusOutput {
    pub state: FocusMonitorState,
    pub command: Option<FocusCommand>,
    pub baseline_sharpness: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HillPhase {
    Idle,
    ProbeStart,
    Climbing,
}

pub struct AutoFocusController {
    baseline: f32,
    low_sharp_elapsed: f32,
    cooldown_remaining: f32,
    hill_phase: HillPhase,
    hill_direction_far: bool,
    hill_start_sharp: f32,
    hill_best_sharp: f32,
    hill_elapsed: f32,
    hill_reversed: bool,
}

impl AutoFocusController {
    pub fn new() -> Self {
        Self {
            baseline: 0.72,
            low_sharp_elapsed: 0.0,
            cooldown_remaining: 0.0,
            hill_phase: HillPhase::Idle,
            hill_direction_far: true,
            hill_start_sharp: 0.0,
            hill_best_sharp: 0.0,
            hill_elapsed: 0.0,
            hill_reversed: false,
        }
    }

    pub fn reset(&mut self) {
        self.low_sharp_elapsed = 0.0;
        self.cooldown_remaining = 0.0;
        self.hill_phase = HillPhase::Idle;
    }

    pub fn update(
        &mut self,
        enabled: bool,
        strategy: AutoFocusStrategy,
        face_sharpness: f32,
        subject_locked: bool,
        ptz_velocity: PtzVelocity,
        subject_speed: f32,
        dt: f32,
    ) -> AutoFocusOutput {
        let dt = dt.clamp(0.005, 0.25);

        if !enabled || !subject_locked || face_sharpness <= 0.001 {
            self.low_sharp_elapsed = 0.0;
            if self.hill_phase != HillPhase::Idle {
                self.hill_phase = HillPhase::Idle;
                return AutoFocusOutput {
                    state: FocusMonitorState::Sharp,
                    command: Some(FocusCommand::Stop),
                    baseline_sharpness: self.baseline,
                };
            }
            return AutoFocusOutput {
                state: FocusMonitorState::Sharp,
                command: None,
                baseline_sharpness: self.baseline,
            };
        }

        // Motion Gate: never mistake camera pan/zoom or fast speaker movement for optical defocus
        let in_motion = ptz_velocity.pan.abs() > 0.06
            || ptz_velocity.tilt.abs() > 0.06
            || ptz_velocity.zoom.abs() > 0.04
            || subject_speed.abs() > 0.14;

        if in_motion {
            self.low_sharp_elapsed = 0.0;
            let stop_cmd = if self.hill_phase != HillPhase::Idle {
                self.hill_phase = HillPhase::Idle;
                Some(FocusCommand::Stop)
            } else {
                None
            };
            return AutoFocusOutput {
                state: FocusMonitorState::MotionGated,
                command: stop_cmd,
                baseline_sharpness: self.baseline,
            };
        }

        // Handle active Tier-2 bounded hill-climb search if in progress
        if self.hill_phase != HillPhase::Idle {
            return self.step_hill_climb(face_sharpness, dt);
        }

        // Cooldown period prevents focus hunting immediately after refocusing
        if self.cooldown_remaining > 0.0 {
            self.cooldown_remaining = (self.cooldown_remaining - dt).max(0.0);
            if face_sharpness > self.baseline {
                self.baseline = self.baseline * 0.85 + face_sharpness * 0.15;
            }
            return AutoFocusOutput {
                state: FocusMonitorState::Cooldown,
                command: None,
                baseline_sharpness: self.baseline,
            };
        }

        // Adaptive stationary threshold: drop below 72% of rolling baseline (capped at 0.45)
        let defocus_threshold = (self.baseline * 0.72).clamp(0.22, 0.48);

        if face_sharpness < defocus_threshold {
            self.low_sharp_elapsed += dt;
            if self.low_sharp_elapsed >= 0.45 {
                self.low_sharp_elapsed = 0.0;
                match strategy {
                    AutoFocusStrategy::OnePushAf => {
                        self.cooldown_remaining = 4.0;
                        return AutoFocusOutput {
                            state: FocusMonitorState::Focusing,
                            command: Some(FocusCommand::OnePush),
                            baseline_sharpness: self.baseline,
                        };
                    }
                    AutoFocusStrategy::ActiveHillClimb => {
                        self.hill_phase = HillPhase::ProbeStart;
                        self.hill_direction_far = true;
                        self.hill_start_sharp = face_sharpness;
                        self.hill_best_sharp = face_sharpness;
                        self.hill_elapsed = 0.0;
                        self.hill_reversed = false;
                        return AutoFocusOutput {
                            state: FocusMonitorState::Focusing,
                            command: Some(FocusCommand::Manual),
                            baseline_sharpness: self.baseline,
                        };
                    }
                }
            }
            return AutoFocusOutput {
                state: FocusMonitorState::Defocused,
                command: None,
                baseline_sharpness: self.baseline,
            };
        }

        // Stationary and sharp: smoothly refresh rolling baseline
        self.low_sharp_elapsed = (self.low_sharp_elapsed - dt * 2.0).max(0.0);
        self.baseline = (self.baseline * 0.96 + face_sharpness * 0.04).clamp(0.35, 0.95);

        AutoFocusOutput {
            state: FocusMonitorState::Sharp,
            command: None,
            baseline_sharpness: self.baseline,
        }
    }

    fn step_hill_climb(&mut self, sharp: f32, dt: f32) -> AutoFocusOutput {
        self.hill_elapsed += dt;

        // Hard safety time-bound (max 2.2s search) so manual focus never wanders away
        if self.hill_elapsed >= 2.2 {
            self.hill_phase = HillPhase::Idle;
            self.cooldown_remaining = 4.5;
            self.baseline = self.baseline.max(self.hill_best_sharp);
            return AutoFocusOutput {
                state: FocusMonitorState::Cooldown,
                command: Some(FocusCommand::Stop),
                baseline_sharpness: self.baseline,
            };
        }

        match self.hill_phase {
            HillPhase::ProbeStart => {
                self.hill_phase = HillPhase::Climbing;
                self.hill_start_sharp = sharp;
                self.hill_best_sharp = sharp;
                let cmd = if self.hill_direction_far {
                    FocusCommand::Far { speed: 0.28 }
                } else {
                    FocusCommand::Near { speed: 0.28 }
                };
                AutoFocusOutput {
                    state: FocusMonitorState::Focusing,
                    command: Some(cmd),
                    baseline_sharpness: self.baseline,
                }
            }
            HillPhase::Climbing => {
                if sharp > self.hill_best_sharp {
                    self.hill_best_sharp = sharp;
                }

                // If sharpness dropped noticeably below our best peak:
                if sharp < self.hill_best_sharp - 0.035 {
                    if !self.hill_reversed && self.hill_best_sharp <= self.hill_start_sharp + 0.02 {
                        // Never rose above starting sharpness -> probed wrong way; reverse once toward peak
                        self.hill_reversed = true;
                        self.hill_direction_far = !self.hill_direction_far;
                        self.hill_start_sharp = sharp;
                        self.hill_best_sharp = sharp;
                        let cmd = if self.hill_direction_far {
                            FocusCommand::Far { speed: 0.35 }
                        } else {
                            FocusCommand::Near { speed: 0.35 }
                        };
                        return AutoFocusOutput {
                            state: FocusMonitorState::Focusing,
                            command: Some(cmd),
                            baseline_sharpness: self.baseline,
                        };
                    }
                    // Passed peak after rising -> stop cleanly at optimal focus!
                    self.hill_phase = HillPhase::Idle;
                    self.cooldown_remaining = 4.5;
                    self.baseline = self.hill_best_sharp.clamp(0.35, 0.95);
                    return AutoFocusOutput {
                        state: FocusMonitorState::Cooldown,
                        command: Some(FocusCommand::Stop),
                        baseline_sharpness: self.baseline,
                    };
                }

                AutoFocusOutput {
                    state: FocusMonitorState::Focusing,
                    command: None,
                    baseline_sharpness: self.baseline,
                }
            }
            HillPhase::Idle => AutoFocusOutput {
                state: FocusMonitorState::Sharp,
                command: None,
                baseline_sharpness: self.baseline,
            },
        }
    }
}

impl Default for AutoFocusController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_gate_prevents_false_autofocus_during_pan() {
        let mut af = AutoFocusController::new();
        for _ in 0..30 {
            let out = af.update(
                true,
                AutoFocusStrategy::OnePushAf,
                0.15, // blurry due to pan motion
                true,
                PtzVelocity { pan: 0.35, tilt: 0.0, zoom: 0.0 },
                0.0,
                0.033,
            );
            assert_eq!(out.state, FocusMonitorState::MotionGated);
            assert!(out.command.is_none());
        }
    }

    #[test]
    fn stationary_defocus_triggers_one_push_and_enforces_cooldown() {
        let mut af = AutoFocusController::new();
        let mut triggered = 0;
        for _ in 0..30 {
            let out = af.update(
                true,
                AutoFocusStrategy::OnePushAf,
                0.20,
                true,
                PtzVelocity::default(),
                0.0,
                0.033,
            );
            if out.command == Some(FocusCommand::OnePush) {
                triggered += 1;
            }
        }
        assert_eq!(triggered, 1, "must trigger OnePush AF once and respect 4s cooldown");
    }

    #[test]
    fn hill_climb_reverses_and_stops_at_optical_focus_peak() {
        let mut af = AutoFocusController::new();
        // Simulated focus ring position (0..10), true peak sharpness at pos = 3.0 (Near direction)
        let mut focus_pos = 5.0_f32;
        let mut focus_vel = 0.0_f32;
        let sharpness_fn = |pos: f32| -> f32 { 0.90 * (-((pos - 3.0) * (pos - 3.0)) / 3.0).exp() };

        let mut stopped_near_peak = false;
        for _ in 0..100 {
            let sharp = sharpness_fn(focus_pos);
            let out = af.update(
                true,
                AutoFocusStrategy::ActiveHillClimb,
                sharp,
                true,
                PtzVelocity::default(),
                0.0,
                0.05,
            );
            if let Some(cmd) = out.command {
                match cmd {
                    FocusCommand::Far { speed } => focus_vel = speed * 4.0,
                    FocusCommand::Near { speed } => focus_vel = -speed * 4.0,
                    FocusCommand::Stop => {
                        stopped_near_peak = (focus_pos - 3.0).abs() < 0.9;
                        break;
                    }
                    _ => {}
                }
            }
            focus_pos += focus_vel * 0.05;
        }
        assert!(
            stopped_near_peak,
            "hill-climb should locate peak near 3.0 and stop cleanly (stopped at {focus_pos:.2})"
        );
    }
}
