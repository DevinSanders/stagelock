# `stagelock-vision`

Real-time multi-person detection, pose estimation, identity-locked tracking (**BoT-SORT + Camera Motion Compensation**), appearance Re-ID, and subject-aware Face-ROI sharpness monitoring.

## Modules

- [`src/onnx_engine.rs`](./src/onnx_engine.rs) — Optional ONNX Runtime (`ort` v2) wrapper for permissive Apache-2.0/MIT models (`RTMDet-s`, `RTMPose-s`, `OSNet x0.25`) with CUDA, DirectML, CoreML, and CPU execution provider support.
- [`src/reid.rs`](./src/reid.rs) — L2-normalized appearance embeddings, zero-dependency 96-dim 3-zone spatial HSV Re-ID fallback, and a 12-slot overlap-frozen `IdentityGallery` that preserves the original operator-clicked anchor across pose changes.
- [`src/cmc.rs`](./src/cmc.rs) — Foreground-masked sparse block-matching Camera Motion Compensation (`CameraMotionCompensator`) to stabilize tracks during rapid PTZ pans.
- [`src/tracker.rs`](./src/tracker.rs) — Two-stage confidence BoT-SORT tracker with fused IoU + Re-ID anti-swap gating, overlap freeze, calm `OccludedHold`, and margin-verified Re-ID re-acquisition.
- [`src/sharpness.rs`](./src/sharpness.rs) — Face/upper-head ROI normalized Tenengrad (Sobel gradient) sharpness estimator immune to sharp stage backgrounds or LED walls.
- [`src/pipeline.rs`](./src/pipeline.rs) — Per-camera stateful `CameraVisionPipeline`.

## Optional Cargo Features

- `onnx`, `onnx-cuda`, `onnx-directml`, `onnx-coreml`

## Tests

```pwsh
cargo test -p stagelock-vision
```

Covers crossing subjects without ID swap, complete occlusion hold & exact re-acquisition, fast-pan CMC tracking, prolonged occlusion expiry to `Searching`, undersized/flat frame resilience, and invalid lock/embedding edge cases.
