# WL-FUNC-019 resource snapshot revocation — 2026-08-11

- Scope: malformed newer resource catalogs revoke prior launch authority and cancel prepared VDI handoffs while retaining cards only for untrusted inspection.
- Hostile boundary: a mismatched replacement snapshot cannot leave an older resource activatable.
- Focused gate: `cargo test -p mde-shell-egui chooser::resources::tests::mismatched_newer_snapshot_cannot_retain_prior_launch_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: prove live corrected-forward catalog recovery after a malformed Bus replacement.
