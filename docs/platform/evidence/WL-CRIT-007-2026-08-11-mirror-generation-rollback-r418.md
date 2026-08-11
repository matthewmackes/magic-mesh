# WL-CRIT-007 mirror generation rollback — 2026-08-11

- Scope: a restarted mirror worker advertises only a generation proven by its current process.
- Hostile boundary: replicated generation rollback retracts DNF readiness until a strictly corrected-forward generation returns.
- Focused gate: `cargo test -p mackesd workers::mirror_syncd::tests::replicated_generation_rollback_retracts_repo_until_corrected_forward_return -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 2, admitted with 15,713,872 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,873 filtered out.
- Remaining boundary: interrupt and return a live Syncthing mirror during role transition and observe installed-node readiness retraction/recovery.
