# WL-FUNC-023 lifecycle-authority farm evidence — 2026-08-16

- Source revision: `d8e811e2a8a77ffc428790ed6e7c6401651c8c23`
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `wl-func023-lifecycle-warm-20260816`
- Command: `target/debug/deps/mackesd_core-2996e67e4f6aede8 lifecycle_authority --nocapture`
- Result: `17 passed, 0 failed, 5004 filtered out`

The focused tests cover target/generation binding, exclusive authority,
atomic checkpoints, interruption/resume, correction planning, fleet report
truthfulness, pinned and unsigned artifact admission, commissioning capsule
retry/revocation, confirmation scope, readiness warnings, terminal progress,
and offboarding receipt completion. This is product-core evidence only; live
SSH bootstrap, live Bus acknowledgement, package integration, and physical
seat acceptance remain open.
