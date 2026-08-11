# WL-FUNC-020 Cuttlefish readiness revocation — 2026-08-11

- Scope: failed guest-readiness or inventory refresh revokes retained launch and
  VDI authority until a corrected-forward refresh succeeds.
- Hostile boundary: provider loss makes a stale-ready guest unavailable before
  backend launch contact; recovered exact readiness restores authority.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::cloud::verbs::android::cuttlefish::tests::failed_readiness_refresh_revokes_retained_launch_authority -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,850 filtered out.
- Remaining boundary: governed guest packaging, nested-KVM, and live-seat proof remain.
