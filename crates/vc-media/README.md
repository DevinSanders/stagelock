# `vc-media`

GPU-accelerated video ingest, **NDI® 6** dynamic runtime receive/re-broadcast, real-time BGRA color grading & Unsharp-Mask sharpening, and per-camera worker supervision.

## Modules

- [`src/capabilities.rs`](./src/capabilities.rs) — Cross-platform GPU, GStreamer plugin, and NDI® 6 runtime prober (`NVIDIA NVDEC/NVENC + CUDA/TensorRT`, `AMD AMF/D3D11 + DirectML`, `Intel QuickSync + OpenVINO`, `Apple VideoToolbox + CoreML`, `Linux VAAPI`, software fallback).
- [`src/ndi.rs`](./src/ndi.rs) — Dynamic `libloading` FFI wrapper for the user-installed NDI® 6 Runtime (`NDIlib_find`, `NDIlib_recv`, `NDIlib_send`, native NDI PTZ & camera imaging).
- [`src/gst_pipeline.rs`](./src/gst_pipeline.rs) & [`src/gst_runner.rs`](./src/gst_runner.rs) — Hardware-accelerated GStreamer capture pipelines (`appsink max-buffers=1 drop=true`, 25 ms RTSP default) for RTSP, RTP, ONVIF, WebRTC (WHEP), USB/UVC (`mfvideosrc`/`avfvideosrc`/`v4l2src`), and Blackmagic DeckLink SDI/HDMI, plus local capture device discovery.
- [`src/h264_preview.rs`](./src/h264_preview.rs) — Lazy per-camera zero-latency GPU H.264 Annex-B encoder (`nvh264enc`, `amfh264enc`, `qsvh264enc`, `vtenc_h264_hw`, `vah264enc`, `mfh264enc`, or BSD `openh264enc`) with SPS/IDR NAL parsing and 20-byte `VCH2` binary WebSocket packet framing for browser WebCodecs `VideoDecoder`.
- [`src/color.rs`](./src/color.rs) — In-place Layer-2 BGRA color grading (White Balance Temp/Tint, Exposure, Contrast, Saturation, Brightness) and 5-tap Laplacian Unsharp-Mask sharpening.
- [`src/test_pattern.rs`](./src/test_pattern.rs) — Animated multi-subject synthetic camera generator for offline testing and demos.
- [`src/engine.rs`](./src/engine.rs) — Per-camera async worker supervisor publishing telemetry immediately after AI vision while decoupling JPEG & GPU H.264 preview encoding onto non-blocking latest-frame workers.

## Optional Cargo Features

- `--features gst-native` — Links in-process `gstreamer-rs` / `gstreamer-app` directly for zero-IPC `AppSink` pulls (requires GStreamer development headers). Without this feature, `vc-media` seamlessly uses the `gst-launch-1.0` / `gst-device-monitor-1.0` runtime bridge.

## Tests

```pwsh
cargo test -p vc-media
```

> **NDI® Notice:** NDI® is a registered trademark of Vizrt NDI AB ([ndi.video](https://ndi.video)). See [`docs/ndi-attribution.md`](../../docs/ndi-attribution.md).
