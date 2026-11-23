# `vc-tracking`

Broadcast-grade smooth PTZ motion control, anatomical Rule-of-Thirds auto-framing, and subject-aware 2-tier motion-gated auto-focus.

## Modules

- [`src/one_euro.rs`](./src/one_euro.rs) — Adaptive One-Euro low-pass filter (heavy smoothing while stationary, low latency during movement).
- [`src/framing.rs`](./src/framing.rs) — Anatomical keypoint framing solver supporting `Close-Up`, `Waist-Up`, and `Full Height` presets plus `Left 1/3`, `Center`, `Right 1/3`, and hysteresis-stabilized `Auto Lead-Room`.
- [`src/motion.rs`](./src/motion.rs) — 5-stage `SmoothMotionController`:
  1. One-Euro filtering
  2. Nested outer/inner dead-zone hysteresis
  3. Zoom-compensated PD + directional velocity feed-forward + **vMix Program tally damping**
  4. S-curve acceleration & jerk limiting (fluid-head feathering)
  5. Decoupled slow optical zoom & per-camera `CameraCalibration` gain mapping
- [`src/autofocus.rs`](./src/autofocus.rs) — 2-tier motion-gated `AutoFocusController` (`OnePushAf` with rolling baseline & cooldown + bounded `ActiveHillClimb` with 2.2s hard safety timeout).
- [`src/controller.rs`](./src/controller.rs) — Per-camera `CameraTrackingController` combining framing, motion, manual-override hold, and auto-focus for independent simultaneous multi-camera tracking.

## Tests

```pwsh
cargo test -p vc-tracking
```

Includes closed-loop 165ms-latency PTZ step-response convergence, 3 simultaneous independent camera tracking, tally damping, motion-gated AF, hill-climb peak tracking & 2.2s hard timeout safety, manual override suppression, and zero/negative `dt` guards.
