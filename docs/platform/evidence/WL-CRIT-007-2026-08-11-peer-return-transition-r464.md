# WL-CRIT-007 peer-return transition authority — 2026-08-11

- Scope: presence reconciliation preserves a current peer-return transition.
- Hostile boundary: an expired peer row cannot erase a newer returned state.
- Focused gate: `cargo test -p mackesd workers::presence_watch::tests::expired_peer_row_cannot_erase_the_return_transition -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Related exact passes: `WL-CRIT-007-2026-08-11-roaming-session-identity-r465.md` and `WL-CRIT-007-2026-08-11-fleet-reconcile-process-budget-r469.md`.
- Remaining boundary: six-node sleep/resume and return capture.
