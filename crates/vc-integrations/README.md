# `vc-integrations`

Broadcast switcher and control surface protocol clients for **VideoController**.

## Modules

- [`src/vmix.rs`](./src/vmix.rs) — **vMix** integration:
  - Persistent TCP Tally & Activators subscriber (`:8099`, `SUBSCRIBE TALLY` / `SUBSCRIBE ACTS`) with automatic reconnect and non-blocking XML metadata polling.
  - HTTP XML API (`:8088/api/`) for input enumeration and live switching (`Cut`, `Fade`, `Stinger1..4`, `PreviewInput`, `ActiveInput`, `OverlayInput1..4`).
  - Camera tally resolver matching input numbers, titles, short titles, or GUIDs.
- [`src/bitfocus_buttons.rs`](./src/bitfocus_buttons.rs) — Bi-directional **Bitfocus Buttons Control API** HTTP client (`/control/...` with optional Bearer API key).
- [`src/streamdeck.rs`](./src/streamdeck.rs) — Unified `StreamDeckAction` router (keys + rotary dials) and Direct USB HID coexistence toggle (disabled by default when Bitfocus Companion/Buttons owns the Stream Deck USB device).

## Tests

```pwsh
cargo test -p vc-integrations
```

Includes live loopback mock-vMix TCP Tally subscription tests, XML state parsing, malformed XML & disconnected tally failure paths, and disabled-client error paths.
