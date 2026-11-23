# VideoController Desktop Studio & Mobile/Tablet Remote PWA

Cross-platform **Electron + React + TypeScript** broadcast studio interface and responsive LAN **Progressive Web App (PWA)** for iPhone, iPad, and Android.

## Architecture

- **Desktop Studio (`electron/`):**
  - [electron/main.cjs](./electron/main.cjs) supervises the local Rust `vc-daemon` binary (`resources/bin/vc-daemon` when packaged, or `target/release/vc-daemon` in dev) and serves the built UI via `app.asar`.
  - [electron/preload.cjs](./electron/preload.cjs) exposes a context-isolated `window.vcHost` bridge pointing to `http://127.0.0.1:8765`.
- **Responsive Web / Mobile PWA (`src/` + `public/`):**
  - The same Vite production bundle (`dist/`) is served directly over LAN by `vc-daemon` at `http://<studio-pc-ip>:8765`.
  - Desktop displays a multi-camera broadcast multiview grid with inspector panels (`AddCameraModal`, `IntegrationsPanel`, `SetupWizardModal`, `RemoteAccessModal` QR code).
  - Phones and tablets automatically adapt to a touch-first single-camera live monitor with tap-to-track subject locking, mobile MJPEG rate limiting (`?max_fps=12`), touch PTZ joystick, zoom/focus controls, presets, and home-screen install support (`manifest.webmanifest` + `sw.js`).

## Directory Layout

| Path | Responsibility |
| :--- | :--- |
| `electron/` | Electron main process supervisor and context-isolated preload bridge |
| `public/` | PWA manifest (`manifest.webmanifest`), service worker (`sw.js`), and vector app icon (`icon.svg`) |
| `src/App.tsx` | Responsive Desktop Studio & Mobile/Tablet Remote controller layout |
| `src/api.ts` | REST, MJPEG, and WebSocket client targeting `vc-daemon` |
| `src/components/` | `CameraMonitor`, `PtzJoystick`, `AddCameraModal`, `RemoteAccessModal`, `IntegrationsPanel`, `SetupWizardModal` |
| `electron-builder.yml` | Installer packaging config for Windows (`.exe` NSIS + portable), macOS (`.dmg`), and Linux (`.AppImage` + `.deb`) |

## Commands

Run from the repository root (or inside `apps/desktop/`):

```pwsh
# Development (Vite dev server + Electron window)
pnpm dev:ui

# Typecheck & build production UI + LAN PWA bundle (outputs to dist/)
pnpm build:ui

# Package platform installers (requires release vc-daemon + Companion module built first)
pnpm package:win    # Windows NSIS Setup .exe + Portable .exe -> release/
pnpm package:mac    # macOS .dmg -> release/
pnpm package:linux  # Linux .AppImage + .deb -> release/
```

See also: [Root README](../../README.md) · [`vc-daemon` API](../../crates/vc-daemon/README.md)
