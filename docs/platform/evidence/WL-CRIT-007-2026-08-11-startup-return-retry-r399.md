# WL-CRIT-007 startup return retry — 2026-08-11

- Scope: daemon startup must eventually reconcile a retained sleep/reboot intent after transient network instability.
- Hostile boundary: one false NetworkManager stability verdict cannot strand the node absent when no later logind return signal will arrive.
- Focused gate: `cargo test -p mackesd workers::host_state::tests::startup_return_retries_after_transient_network_instability -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 12,848,568 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,867 filtered out.
- Remaining boundary: physical suspend/reboot with a failed first stability probe and later live `Returned` publication remains.
