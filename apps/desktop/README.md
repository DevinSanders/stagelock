# VideoController Desktop Studio (Tauri 2) & Mobile/Tablet Remote PWA

Cross-platform **Tauri 2 + React + TypeScript** broadcast studio application and responsive LAN **Progressive Web App (PWA)** for iPhone, iPad, and Android.

## Architecture

- **Native Tauri 2 Shell (`src-tauri/`):**
  - Embeds [`vc-daemon`](../../crates/vc-daemon/README.md) **in-process** inside a single compact Rust executable (zero child-process spawning, zero orphan-process leaks, instant SQLite WAL checkpoint on `RunEvent::Exit`).
  - Uses OS WebViews (**WebView2** on Windows, **WKWebView** on macOS, **WebKitGTK** on Linux) for low idle RAM overhead alongside vMix, NDI Tools, and ONNX Runtime.
  - Set `VC_EXTERNAL_DAEMON=1` if you want the desktop UI to attach to a separate headless `vc-daemon` instance instead.
- **Ultra-Low-Latency Video Preview (`WebCodecsVideo.tsx`):**
  - Connects to `ws://<host>:8765/ws/video/{id}` and decodes GPU zero-latency H.264 Annex-B (`nvh264enc`/`amfh264enc`/`qsvh264enc`/`vtenc_h264_hw`/`mfh264enc`/`openh264enc`) directly via browser **WebCodecs `VideoDecoder`** (`optimizeForLatency: true`) onto `<canvas>`.
  - Transparently falls back to rate-limited MJPEG (`/stream.mjpeg?max_fps=12`) on browsers or hosts without WebCodecs/H.264.
- **Responsive LAN Mobile/Tablet Remote PWA (`src/` + `public/`):**
  - The same Vite production bundle (`dist/`) is bundled as `web/` and served directly over LAN by the embedded daemon at `http://<studio-pc-ip>:8765` with QR code modal, tap-to-track monitor, touch joystick, and home-screen install support.

## Commands

Run from the repository root:

```pwsh
# Development (Vite dev server + Tauri 2 desktop window with embedded vc-daemon)
pnpm dev:ui

# Typecheck & build production UI + LAN PWA bundle (outputs to dist/)
pnpm build:ui

# Run user-perspective UI tests (Vitest + React Testing Library) & >=90% V8 coverage gate
pnpm test:ui
pnpm test:ui:coverage

# Package native Tauri 2 platform installers (outputs to target/release/bundle/)
pnpm package:win    # Windows NSIS .exe + .msi
pnpm package:mac    # macOS .dmg
pnpm package:linux  # Linux .AppImage + .deb
```

See also: [Root README](../../README.md) · [`vc-daemon` API](../../crates/vc-daemon/README.md)
