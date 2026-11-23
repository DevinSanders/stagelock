# Bitfocus Companion / Buttons Module (`companion-module-videocontroller`)

First-party TypeScript module built on `@companion-module/base` for controlling **VideoController** from **Bitfocus Companion**, **Bitfocus Buttons**, Elgato **Stream Deck**, and **Stream Deck +** (including rotary dials).

## Features

- **Real-Time WebSocket Feedback (`ws://<host>:8765/ws/telemetry`):** Live button color/text updates for **vMix Program (Red) / Preview (Green) Tally**, **AI Tracking State** (`Locked`, `OccludedHold`, `Searching`, `Idle`), **Auto-Focus State**, and **NDI® Output Active**.
- **Actions:**
  - Toggle AI Auto-Tracking per camera
  - Set Framing Preset (`Close-Up`, `Waist-Up`, `Full Height`)
  - Set Rule-of-Thirds Composition (`Left 1/3`, `Center`, `Right 1/3`, `Auto Lead-Room`)
  - Trigger One-Push Auto-Focus
  - Recall / Store PTZ Presets (slots 1–16)
  - Toggle NDI® Re-Broadcast per camera
  - **Stream Deck + Rotary Dials:** Smooth incremental Zoom, Focus, Exposure, and White Balance adjustments
  - vMix Cut / Fade switching
- **Dynamic Variables & Drag-and-Drop Presets:** Automatically populated from your VideoController camera list on connect.

## Building

From the repository root:

```pwsh
pnpm install
pnpm --filter companion-module-videocontroller build
```

Compiled JavaScript outputs to `dist/` alongside the module metadata in `companion/manifest.json`.

## Loading in Bitfocus Companion (Developer Mode)

1. Build the module (`pnpm --filter companion-module-videocontroller build`).
2. In Bitfocus Companion settings, point the **Developer Modules Path** to the parent `integrations/` directory (or copy `dist/`, `companion/`, and `package.json` into a module folder).
3. Add a **VideoController** connection pointing to your studio PC host and port (`127.0.0.1:8765` locally or your LAN IP).

> **Tip:** When using Bitfocus Companion or Bitfocus Buttons with a Stream Deck, leave **Direct USB Stream Deck** disabled in VideoController's Integrations panel so Bitfocus retains exclusive USB HID access.
