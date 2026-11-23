# VideoController

Cross-platform, GPU-accelerated multi-camera preview, **NDI® re-broadcast**, **AI subject auto-tracking & broadcast framing**, **vMix live tally/switching**, **Stream Deck + Bitfocus Companion/Buttons**, and **Mobile/Tablet Remote PWA** studio controller built with **Rust + GStreamer + ONNX Runtime (`ort`)** and **Electron (React + TypeScript)**.

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

## Workspace Architecture

| Crate / App | Responsibility |
| :--- | :--- |
| `crates/vc-core` | Shared domain models, UUID camera configs, telemetry schemas, embedded SQLite (`videocontroller.db`) store with versioned migrations & legacy `config.json` auto-import |
| `crates/vc-media` | Multi-GPU prober, GStreamer live ingest (`gst-native` or runtime bridge), NDI® 6 dynamic loader, color & Unsharp-Mask pipeline |
| `crates/vc-vision` | ONNX Runtime (`ort` v2) wrapper (`RTMDet`, `RTMPose`, `OSNet`), spatial HSV fallback, CMC, BoT-SORT, identity gallery, face-ROI sharpness |
| `crates/vc-tracking` | One-Euro filter, anatomical broadcast framing solver, 5-stage smooth PTZ controller, 2-tier motion-gated auto-focus |
| `crates/vc-ptz` | Unified PTZ/Imaging trait + VISCA-IP, ONVIF S/T + WS-Discovery, NDI PTZ, Canon XC, Panasonic AW, Mock PTZ |
| `crates/vc-integrations` | vMix TCP Tally + HTTP switching client, Bitfocus Buttons Control API client, Stream Deck key/dial router |
| `crates/vc-daemon` | Headless Rust control & media server (REST + 30 Hz WebSocket + MJPEG + PWA static server + 30 Hz tracking supervisor + OSC `:9000`) |
| `apps/desktop` | Electron + React + TypeScript multiview studio & responsive Mobile/Tablet Remote PWA |
| `integrations/companion-module` | First-party Bitfocus Companion / Buttons module (`@companion-module/base`) |

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
4. **Build Installers (Windows `.exe`, macOS `.dmg`, Linux `.AppImage` + `.deb`):**
   ```pwsh
   # Windows (builds vc-daemon release, UI/PWA, Companion module, NSIS Setup .exe + Portable .exe):
   .\scripts\package-windows.ps1

   # Cross-platform via electron-builder (after `cargo build --release -p vc-daemon` and `pnpm build:ui`):
   pnpm package:win    # Windows NSIS Setup .exe + Portable .exe (apps/desktop/release/)
   pnpm package:mac    # macOS .dmg (apps/desktop/release/)
   pnpm package:linux  # Linux .AppImage + .deb (apps/desktop/release/)
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
