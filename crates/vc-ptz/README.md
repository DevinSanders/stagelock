# `vc-ptz`

Unified async PTZ, Focus, Preset, and Layer-1 Camera Hardware Imaging drivers across broadcast camera protocols.

## Supported Protocols

| Module | Protocol | Details |
| :--- | :--- | :--- |
| [`src/visca.rs`](./src/visca.rs) | **VISCA over IP** | Sony UDP (`:52381` with 8-byte header + sequence counter), Raw TCP (`:5678` — PTZOptics, BirdDog, Marshall, Lumens), Raw UDP (`:1259` — PTZOptics, AVer), plus VISCA hardware WB/Exposure/Backlight/Sharpness/Contrast/Brightness |
| [`src/onvif.rs`](./src/onvif.rs) | **ONVIF Profile S / T** | WS-Discovery LAN scanner (`239.255.255.250:3702`), WS-Security `PasswordDigest` auth, automatic `GetCapabilities` + `GetProfiles` + `GetStreamUri` RTSP resolution, `ContinuousMove`/`Stop`/`GotoPreset`/`SetPreset`, and Imaging service control |
| [`src/ndi_ptz.rs`](./src/ndi_ptz.rs) | **NDI® Native PTZ** | Binds directly to the active `vc-media` NDI 6 receiver for PTZ, Focus, Presets, WB, and Exposure |
| [`src/http_drivers.rs`](./src/http_drivers.rs) | **Canon XC & Panasonic AW** | HTTP/CGI drivers for Canon CR-N series (`/-wvhttp-01-/control.cgi`) and Panasonic AW-UE/HE series (`/cgi-bin/aw_ptz` & `/cgi-bin/aw_cam`) |
| [`src/mock.rs`](./src/mock.rs) | **Mock PTZ** | Software physics-simulated PTZ camera for offline setup and testing |
| [`src/manager.rs`](./src/manager.rs) | **`PtzManager`** | Per-camera driver registry synchronized with `CameraConfig` |

## Tests

```pwsh
cargo test -p vc-ptz
```

Verifies VISCA byte encodings & clamping, Sony header framing, ONVIF WS-Security SHA-1 digest, WS-Discovery/Profile/URI XML parsing, malformed XML resilience, Panasonic AW speed clamping, and `NdiPtzDriver` error handling when a worker is missing or non-NDI.
