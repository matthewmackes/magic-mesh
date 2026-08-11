# WL-FUNC-020 Cuttlefish exchange generation — 2026-08-11

- Scope: Cuttlefish inventory and VDI observations originate within the current guest exchange.
- Hostile boundary: pre-restart inventory cannot be relabeled under a fresh response envelope.
- Focused gate: `cargo test -p mackesd workers::cloud::verbs::android::cuttlefish_guest::tests::pre_restart_inventory_cannot_authorize_the_current_guest_exchange -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 11,860,940 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,884 filtered out.
- Remaining boundary: restart a live guest exchange and prove only post-request inventory can authorize current VDI readiness.
