## Summary

<!-- One or two sentences on what this change does and why. -->

## Related issue

<!-- e.g. Closes #123, Refs #456. Skip if not applicable. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor / cleanup
- [ ] Documentation
- [ ] Build / CI / tooling

## How I tested this

<!-- What you actually did to verify the change. If UI- or media-affecting, mention the OS, GPU, and camera/PTZ protocol tested. -->

## Project-specific checklist

<!-- Easy-to-miss conventions for StageLock. See CONTRIBUTING.md for details. -->

- [ ] `cargo test --all-targets` passes locally (happy and failure paths covered)
- [ ] `pnpm test:ui:coverage` passes locally (`>= 90%` coverage gate) and `pnpm build:ui` succeeds
- [ ] `pnpm --filter companion-module-stagelock build` succeeds if Companion code changed
- [ ] New dependencies and AI weights are GPL-3.0-compatible (preferring MIT, Apache-2.0, BSD, or LGPL; no AGPL-3.0 weights)
- [ ] If SQLite schema changed in `crates/stagelock-core/src/store.rs`, a versioned migration was added cleanly
- [ ] Mandatory Vizrt NDI® trademark notices (`docs/ndi-attribution.md`, `ndi.video`) remain intact
