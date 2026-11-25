# `stagelock-daemon`

Headless Rust control plane, media server, 30 Hz multi-camera tracking supervisor, OSC controller, and LAN Remote PWA host for **StageLock**.

## Responsibilities

- **Embeddable Library + Standalone Binary (`src/lib.rs` + `src/main.rs`):** Runs embedded in-process inside the Tauri 2 desktop app (`stagelock_daemon::run_daemon`) or standalone as a headless server binary.
- **REST API (`/api/v1/...`):** Camera CRUD, framing/color/NDI/vMix settings, PTZ move/stop/focus/preset/calibration, ONVIF/NDI/USB discovery, vMix & Bitfocus Buttons triggers, LAN remote access metadata (`/api/v1/remote-info`), and graceful teardown (`/api/v1/shutdown`).
- **Real-Time Telemetry WebSocket (`/ws/telemetry`):** Pushes system capabilities on connect followed by ~30 Hz per-camera bounding boxes, keypoints, tracking lock state, focus sharpness, and live vMix tally.
- **Live Preview Streaming:** Ultra-low-latency binary GPU H.264 WebCodecs stream (`/ws/video/{id}`), single-frame JPEG (`/api/v1/cameras/{id}/snapshot.jpg`), and rate-limited MJPEG fallback (`/api/v1/cameras/{id}/stream.mjpeg?max_fps=12`).
- **LAN Mobile/Tablet Remote PWA Server:** Automatically serves the built UI from `apps/desktop/dist` (or `STAGELOCK_WEB_DIST_DIR`) over LAN (`http://<studio-pc-ip>:8765`).
- **Event-Driven Tracking Supervisor ([`src/tracking_loop.rs`](./src/tracking_loop.rs)):** Wakes immediately (`<0.2 ms`) on every fresh vision detection per PTZ camera and persists PTZ speed/latency calibration sweeps into SQLite.
- **OSC UDP Listener (`:9000`):**
  - `/cam/{idx}/track/toggle`
  - `/cam/{idx}/framing/preset` (`close_up` | `waist_up` | `full_height`)
  - `/cam/{idx}/framing/thirds` (`left_third` | `center` | `right_third` | `auto_lead_room`)
  - `/cam/{idx}/focus/one_push`
  - `/cam/{idx}/preset/recall` (`int slot`)
  - `/cam/{idx}/ndi/toggle`
  - `/vmix/cut`, `/vmix/fade`

## Environment Variables

| Variable | Default | Purpose |
| :--- | :--- | :--- |
| `STAGELOCK_CONFIG_PATH` | OS config dir `StageLock/stagelock.db` | Override SQLite database path |
| `STAGELOCK_BIND_ADDR` | `0.0.0.0` (when LAN enabled) | Override HTTP bind interface |
| `STAGELOCK_WEB_DIST_DIR` | Auto-discovered (`web/`, `apps/desktop/dist`) | Directory containing built PWA `index.html` |
| `RUST_LOG` | `stagelock_daemon=info,stagelock_media=info,stagelock_ptz=info,tower_http=info` | Tracing filter |

## Tests

```pwsh
cargo test -p stagelock-daemon
```

Verifies Axum HTTP happy paths (`200`/`201`/`204`) and failure status codes (`404` missing cameras, `400` malformed UUID/JSON & unconfigured PTZ calibration, `502` disabled vMix/Bitfocus Buttons commands).
