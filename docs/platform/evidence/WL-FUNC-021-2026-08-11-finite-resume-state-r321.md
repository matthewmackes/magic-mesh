# WL-FUNC-021 finite resume state — 2026-08-11

- Scope: resume positions and durations must be finite before replacing valid
  durable playback state.
- Hostile boundary: NaN and infinite samples fail closed without poisoning the
  last valid state across serialization and restart.
- Focused gate: `cargo test -p mde-media-core resume::tests::non_finite_samples_cannot_poison_valid_resume_across_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted above the 8 GiB reserve.
- Result: **PASS**, 1 passed, 0 failed, 265 filtered out.
- Remaining boundary: live audible resume and installed-player proof remain.
