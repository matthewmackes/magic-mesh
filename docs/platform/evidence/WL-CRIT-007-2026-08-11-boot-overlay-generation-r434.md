# WL-CRIT-007 boot overlay generation — 2026-08-11

- Scope: boot readiness binds one stable overlay-marker generation to one authoritative local directory identity.
- Hostile boundary: retained identity, marker replacement, or duplicate overlay-IP ownership cannot join readiness.
- Focused gate: `cargo test -p mackesd workers::boot_readiness::tests::replaced_overlay_generation_cannot_join_retained_directory_identity -- --exact --nocapture`.
- Farm: clean rerun on `172.20.0.170`, slot 1, admitted with 11,084,492 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,883 filtered out.
- Remaining boundary: replace the live overlay marker during boot and prove readiness waits for corrected-forward directory convergence.
