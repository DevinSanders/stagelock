# `vc-core`

Shared domain models, real-time telemetry contracts, and embedded **SQLite** configuration persistence for **VideoController**.

## Modules

- [`src/camera.rs`](./src/camera.rs) — `CameraConfig`, `VideoSourceConfig` (NDI, RTSP, RTP, ONVIF, WebRTC WHEP, USB, DeckLink, TestPattern), `PtzConfig`, `NdiOutputConfig`, `FramingConfig`, `ColorAdjustments`, `CameraCalibration`, `CameraPreset`, `VmixBinding`.
- [`src/telemetry.rs`](./src/telemetry.rs) — `CameraTelemetry`, `DetectedSubject`, `Keypoint2D`, `NormalizedRect`, `TrackingState`, `FocusMonitorState`, `TallyState`, `SystemCapabilities`, `DaemonWsEvent`.
- [`src/store.rs`](./src/store.rs) — Embedded SQLite (`rusqlite` bundled) `ConfigStore`.

## SQLite Persistence (`videocontroller.db`)

- **Default Paths:**
  - Windows: `%APPDATA%\VideoController\videocontroller.db`
  - macOS: `~/Library/Application Support/VideoController/videocontroller.db`
  - Linux: `~/.config/VideoController/videocontroller.db`
  - Override via `VC_CONFIG_PATH`.
- **Durability & Schema:** Enables `PRAGMA journal_mode = WAL`, `PRAGMA foreign_keys = ON`, `PRAGMA synchronous = NORMAL`, and `PRAGMA busy_timeout = 5000`. Uses versioned `schema_migrations` across `instance_settings`, `cameras`, and `camera_presets` (`ON DELETE CASCADE`).
- **Legacy Migration:** Automatically imports any sibling `config.json` on first run.

## Tests

```pwsh
cargo test -p vc-core
```

Covers CRUD & calibration roundtrips, `ON DELETE CASCADE` preset cleanup, legacy `config.json` auto-import, corrupt SQLite file rejection, corrupt row JSON detection, strict vs. fallback corrupt JSON handling, and nonexistent UUID mutations.
