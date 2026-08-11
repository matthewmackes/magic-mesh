# WL-FUNC-021 zero-audio provider failover evidence — 2026-08-11

- Scope: a syntactically valid provider stream must produce admitted audio to
  win playback authority. A decoded stream that emits zero audio can no longer
  suppress a healthy fallback provider.
- Hostile boundary: the production decoder path receives a zero-audio first
  provider and a healthy second provider. Failover preserves one logical queue
  boundary and never replays audio already emitted to the renderer.
- Focused gate: `cargo test -p mde-musicd engine::tests::zero_audio_provider_cannot_suppress_healthy_admitted_fallback -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 248 filtered out.
- Remaining boundary: physical audible continuity, renderer/cast/handoff, and
  live release acceptance remain open.
