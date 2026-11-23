# Automation Scripts (`scripts/`)

PowerShell developer, setup, packaging, and verification scripts for Windows.

| Script | Purpose | Example Usage |
| :--- | :--- | :--- |
| [`setup-windows.ps1`](./setup-windows.ps1) | Installs Windows build prerequisites (Rust, Node.js, pnpm, GStreamer 1.28 MSVC unified runtime + dev headers, Visual Studio Build Tools). Run from an elevated PowerShell prompt. | `.\scripts\setup-windows.ps1` |
| [`download-models.ps1`](./download-models.ps1) | Downloads permissive Apache-2.0 ONNX vision weights (`RTMDet-s`, `RTMPose-s`) into [`../models/`](../models/README.md). | `.\scripts\download-models.ps1` |
| [`dev.ps1`](./dev.ps1) | Builds & starts `vc-daemon` alongside the Vite + Electron Desktop Studio (with LAN Remote PWA enabled) and repairs Electron cache extraction if needed. | `.\scripts\dev.ps1` |
| [`package-windows.ps1`](./package-windows.ps1) | Builds the release `vc-daemon.exe`, UI/PWA, Companion module, portable staging folder (`dist-release/windows-x64/`), and Windows NSIS Setup `.exe` + Portable `.exe` installers (`apps/desktop/release/`). Pass `-SkipInstaller` to build only the headless staging bundle. | `.\scripts\package-windows.ps1` |
| [`soak-test.ps1`](./soak-test.ps1) | Launches an isolated 4-camera live reliability test against the release `vc-daemon.exe` verifying SQLite `.db` persistence, 30 fps JPEG delivery, RAM stability, and zero thread leaks. | `.\scripts\soak-test.ps1 -DurationSeconds 30` |

## Running Scripts

If PowerShell script execution is restricted in your current terminal, bypass it for the active session only:

```pwsh
Set-ExecutionPolicy -Scope Process Bypass -Force
```
