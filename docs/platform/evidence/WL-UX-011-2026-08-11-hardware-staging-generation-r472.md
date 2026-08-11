# WL-UX-011 hardware staging generation — 2026-08-11

- Scope: hardware publication consumes only the staging inode opened for the current probe.
- Hostile boundary: path substitution cannot redirect the hardware projection before publication.
- Focused gate: `cargo test -p mackesd workers::hardware_probe::tests::substituted_probe_staging_row_cannot_redirect_hardware_publication -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live provider controls and hardware capture.
