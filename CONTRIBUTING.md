# Contributing to StageLock

Thank you for your interest in contributing to **StageLock — AI PTZ Tracking & Broadcast Studio**!

## Development Prerequisites

- **Rust** (stable toolchain, plus `rustup component add llvm-tools-preview` for local coverage reports)
- **Node.js 22+** and **pnpm 12+**
- **Optional (recommended for full native media):** GStreamer 1.28+ runtime/dev headers and Vizrt NDI® 6 Runtime ([ndi.video/tools](https://ndi.video/tools))

On Windows, run `.\scripts\setup-windows.ps1` from an elevated PowerShell prompt to install build prerequisites automatically.

## Running Locally

```pwsh
# Launch Tauri 2 Desktop Studio with embedded stagelock-daemon + LAN Remote PWA
.\scripts\dev.ps1

# Build Desktop UI + Remote PWA and Bitfocus Companion module
pnpm build:ui
pnpm --filter companion-module-stagelock build
```

## Testing & Coverage Standards

StageLock targets **≥ 90% unit and UI test coverage** across both happy paths and failure paths:

```pwsh
# Run all Rust workspace tests
cargo test --all-targets

# Run user-perspective Frontend UI tests & >= 90% V8 coverage gate
pnpm test:ui:coverage

# Run unified Rust LLVM + Frontend V8 >= 90% coverage gate (Windows)
.\scripts\coverage.ps1 -MinCoverage 90
```

## Key Conventions

- **Permissive Licensing Only:** All code, dependencies, and AI vision weights must remain compatible with `MIT OR Apache-2.0` (no GPL/AGPL crates, codecs such as `x264`, or AGPL vision weights).
- **SQLite Persistence:** Schema changes in `crates/stagelock-core/src/store.rs` must use versioned migrations and keep WAL mode intact.
- **NDI® Attribution:** Preserve Vizrt NDI® trademark notices (`docs/ndi-attribution.md` and `ndi.video` references).
- **Pull Requests:** Open PRs against `main`. CI runs on Windows, macOS, and Linux and must pass before merging.
