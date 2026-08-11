# WL-FUNC-020 expired catalog Bus-replacement recovery — 2026-08-11

- Scope: Android catalog recovery revalidates expiry whenever replacement Bus storage activates.
- Hostile boundary: an expired retained catalog remains only an anti-rollback anchor and cannot replay app/readiness authority into the new Bus.
- Focused gate: `cargo test -p mackesd workers::android_catalog::tests::expired_catalog_cannot_replay_into_replaced_bus -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 16,886,844 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,857 filtered out.
- Remaining boundary: signed AOSP/Cuttlefish install and live nested-KVM package/VDI lifecycle proof remain.
