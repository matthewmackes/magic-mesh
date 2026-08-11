# WL-FUNC-021 audible fallback authority — 2026-08-11

- Scope: buffered bytes do not establish audible authority; a candidate becomes
  authoritative only after a frame is physically rendered.
- Hostile boundary: loss of an inaudible candidate removes it, preserves the
  preceding track's queued tail and playhead, and admits the healthy fallback.
- Focused gate: `cargo test -p mde-musicd engine::tests::buffered_but_inaudible_source_loss_cannot_suppress_admitted_fallback -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2, admitted with 12,212,312 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 261 filtered out.
- Remaining boundary: live audible provider-loss continuation remains.
