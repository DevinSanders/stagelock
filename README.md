# VideoController

Cross-platform, GPU-accelerated multi-camera preview (**zero-latency GPU H.264 WebCodecs + MJPEG fallback**), **NDI® re-broadcast**, **event-driven AI subject auto-tracking & broadcast framing**, **vMix live tally/switching**, **Stream Deck + Bitfocus Companion/Buttons**, and **Mobile/Tablet Remote PWA** studio controller built with **Rust + GStreamer + ONNX Runtime (`ort`)** and **Tauri 2 (React + TypeScript)**.

Licensed under **MIT OR Apache-2.0** ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).

> **NDI® Notice:** NDI® is a registered trademark of Vizrt NDI AB. Learn more at [ndi.video](https://ndi.video) or download NDI Tools at [ndi.video/tools](https://ndi.video/tools). See [docs/ndi-attribution.md](docs/ndi-attribution.md).

---

## Core Capabilities

- **Multi-GPU Hardware Acceleration:** Auto-detects **NVIDIA NVDEC/NVENC + CUDA/TensorRT**, **AMD AMF/D3D11 + DirectML**, **Intel QuickSync + OpenVINO**, **Apple VideoToolbox + CoreML**, and **Linux VAAPI** with permissive software fallbacks.
- **Universal Video Ingest + NDI® Re-Broadcast:**
  - **IP Sources:** NDI® / NDI|HX (dynamic NDI 6 runtime + LAN Finder), RTSP, RTP, ONVIF Profile S/T (WS-Discovery + automatic RTSP resolution), and WebRTC (WHEP).
  - **Physical Capture:** USB (UVC) webcams, USB HDMI capture dongles, virtual capture feeds (e.g., vMix Video), and Blackmagic DeckLink (SDI/HDMI).
  - **Re-Broadcast Any Source Over NDI®:** One-click NDI sender per camera, optionally baking in real-time color grading & Unsharp-Mask sharpness.
- **Standout AI Subject Tracking & Broadcast Framing (`vc-vision` + `vc-tracking`):**
  - **Multi-Subject Identity Locking:** `RTMDet` + `RTMPose` (17 COCO keypoints) + `OSNet`/Spatial HSV Re-ID + **BoT-SORT with Camera Motion Compensation (CMC)** + overlap-frozen multi-angle identity gallery + calm Occluded Hold (never jumps to bystanders during crossings).
  - **Anatomical Rule-of-Thirds Framing:** **Close-Up**, **Waist Up**, and **Full Height** presets + **Left 1/3**, **Center**, **Right 1/3**, and hysteresis-stabilized **Auto Lead-Room**.
  - **Zero-Jitter 5-Stage PTZ Motion:** Adaptive **One-Euro Filter** → Nested Dead-Zone Hysteresis → Zoom-Compensated PD + Velocity Feed-Forward → S-Curve Acceleration/Jerk Limiting → Decoupled Slow Zoom + **vMix Tally-Aware On-Air Damping** + PTZ Calibration Wizard.
  - **Subject-Aware 2-Tier Auto-Focus:** Face-ROI Tenengrad sharpness monitor with motion gating + **Tier 1 Smart One-Push AF** and **Tier 2 Bounded Active Hill-Climb Manual Focus**.
  - **Multi-Camera Independence:** Every PTZ camera runs its own isolated vision & 30 Hz closed-loop tracking worker simultaneously.
- **PTZ Protocols & Dual-Layer Video Adjustments (`vc-ptz` + `vc-media`):**
  - **VISCA over IP** (Sony UDP `:52381`, Raw TCP `:5678`, Raw UDP `:1259`), **ONVIF Profile S/T**, **NDI® Native PTZ**, **Canon XC**, **Panasonic AW**, and physics-simulated **Mock PTZ**.
  - **Layer 1 (Camera Hardware Imaging):** WB Auto/Indoor/Outdoor/One-Push/Manual, Exposure Auto/Manual, Backlight Comp, Hardware Sharpness.
  - **Layer 2 (Live Video Grading):** Real-time WB Temp/Tint, Exposure, Contrast, Saturation, Brightness, and 5-tap Laplacian Unsharp-Mask Sharpness.
- **Broadcast Ecosystem Integrations (`vc-integrations` + `integrations/companion-module`):**
  - **vMix:** Live TCP Tally (`:8099`) for Red Program / Green Preview monitor borders + tally-aware damping, plus HTTP/TCP switching (`Cut`, `Fade`, `Stinger1`, `PreviewInput`, overlays).
  - **Bitfocus Companion & Buttons:** First-party TypeScript module (`@companion-module/base`) with real-time WebSocket feedbacks, variables, presets, and **Stream Deck + rotary dials** + optional Bitfocus Buttons Control API client + full **OSC UDP (`:9000`)** listener.
  - **Direct USB Stream Deck Toggle:** Clean coexistence mode (disabled by default when Bitfocus owns the USB device).
- **Mobile & Tablet Remote PWA (iPhone / iPad / Android):**
  - Served directly from `vc-daemon` on LAN (`http://<studio-pc-ip>:8765`) with a scannable **Remote QR** modal in the desktop app, touch-first joystick/zoom controls, tap-to-track monitor, mobile MJPEG rate throttling, and home-screen install support.

---

## Workspace & Repository Structure

| Path | Responsibility |
| :--- | :--- |
| [`crates/`](crates/README.md) | Rust workspace containing all 7 headless core, media, AI, PTZ, integration, and daemon crates |
| [`crates/vc-core`](crates/vc-core/README.md) | Shared domain models, UUID camera configs, telemetry schemas, embedded SQLite (`videocontroller.db`) store with versioned migrations & legacy `config.json` auto-import |
| [`crates/vc-media`](crates/vc-media/README.md) | Multi-GPU prober, GStreamer live ingest (`gst-native` or runtime bridge), lazy GPU zero-latency H.264 WebCodecs encoder, NDI® 6 dynamic loader, decoupled preview & color/Unsharp-Mask pipeline |
| [`crates/vc-vision`](crates/vc-vision/README.md) | ONNX Runtime (`ort` v2) wrapper (`RTMDet`, `RTMPose`, `OSNet`), spatial HSV fallback, CMC, BoT-SORT, identity gallery, face-ROI sharpness |
| [`crates/vc-tracking`](crates/vc-tracking/README.md) | One-Euro filter, anatomical broadcast framing solver, 5-stage smooth PTZ controller, 2-tier motion-gated auto-focus |
| [`crates/vc-ptz`](crates/vc-ptz/README.md) | Unified PTZ/Imaging trait + VISCA-IP, ONVIF S/T + WS-Discovery, NDI PTZ, Canon XC, Panasonic AW, Mock PTZ |
| [`crates/vc-integrations`](crates/vc-integrations/README.md) | vMix TCP Tally + HTTP switching client, Bitfocus Buttons Control API client, Stream Deck key/dial router |
| [`crates/vc-daemon`](crates/vc-daemon/README.md) | Embeddable Rust control & media server (REST + 30 Hz WS telemetry + binary H.264 WebCodecs WS + MJPEG + PWA static server + event-driven tracking supervisor + OSC `:9000`) |
| [`apps/desktop`](apps/desktop/README.md) | **Tauri 2** + React + TypeScript multiview studio (embeds `vc-daemon` in-process) & responsive Mobile/Tablet Remote PWA ([`apps/`](apps/README.md)) |
| [`integrations/companion-module`](integrations/companion-module/README.md) | First-party Bitfocus Companion / Buttons module (`@companion-module/base`) ([`integrations/`](integrations/README.md)) |
| [`models/`](models/README.md) | Optional permissive Apache-2.0/MIT ONNX weights (`RTMDet-s`, `RTMPose-s`, `OSNet`) & [`MODELS_LICENSE.md`](models/MODELS_LICENSE.md) |
| [`scripts/`](scripts/README.md) | Windows setup, model download, dev launcher, installer packaging, and 4-camera soak test scripts |
| [`docs/`](docs/README.md) | Documentation index and mandatory [`ndi-attribution.md`](docs/ndi-attribution.md) notices |

---

## Quick Start (Windows)

1. **Install Prerequisites (Admin PowerShell):**
   ```pwsh
   Set-ExecutionPolicy -Scope Process Bypass -Force
   .\scripts\setup-windows.ps1
   ```
2. **Optional — Download Permissive Apache-2.0 ONNX Models (`RTMDet-s` + `RTMPose-s`):**
   ```pwsh
   .\scripts\download-models.ps1
   ```
3. **Launch Desktop Studio + LAN Remote PWA:**
   ```pwsh
   .\scripts\dev.ps1
   ```
4. **Build Installers (Windows `.exe`/`.msi`, macOS `.dmg`, Linux `.AppImage` + `.deb`):**
   ```pwsh
   # Windows (builds headless vc-daemon, UI/PWA, Companion module, and Tauri 2 NSIS .exe + MSI):
   .\scripts\package-windows.ps1

   # Cross-platform via Tauri 2 CLI:
   pnpm package:win    # Windows NSIS .exe + .msi (target/release/bundle/)
   pnpm package:mac    # macOS .dmg (target/release/bundle/dmg/)
   pnpm package:linux  # Linux .AppImage + .deb (target/release/bundle/)
   ```
5. **Run Automated 4-Camera Live Soak & Reliability Test:**
   ```pwsh
   .\scripts\soak-test.ps1 -DurationSeconds 30
   ```

## Settings Persistence (SQLite)

Instance settings, camera sources, framing/color/NDI profiles, PTZ calibration profiles, and camera presets are stored in an embedded **SQLite** database (WAL mode, foreign-key enforcement, versioned migrations):

- **Windows:** `%APPDATA%\VideoController\videocontroller.db`
- **macOS:** `~/Library/Application Support/VideoController/videocontroller.db`
- **Linux:** `~/.config/VideoController/videocontroller.db`

Override the path via `VC_CONFIG_PATH`. Any legacy `config.json` alongside the database is imported automatically on first run.

## Automated GitHub Releases

The [Release Installers](.github/workflows/release.yml) workflow runs **only** when a semantic version tag (`vX.Y.Z` or `vX.Y.Z-prerelease`) is pushed. It runs the Rust test suite, builds native installers across Windows (`.exe` NSIS Setup + Portable), macOS (`.dmg`), and Linux (`.AppImage` + `.deb`) plus the Bitfocus Companion module archive, and publishes a GitHub Release automatically:

```pwsh
git tag v0.1.0
git push origin v0.1.0
```
