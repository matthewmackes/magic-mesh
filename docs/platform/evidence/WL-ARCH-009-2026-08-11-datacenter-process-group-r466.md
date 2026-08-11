# WL-ARCH-009 datacenter process-group authority — 2026-08-11

- Scope: datacenter provider execution is bounded to the owning worker process group.
- Hostile boundary: a daemonized provider descendant cannot outlive or pin the datacenter worker group.
- Focused gate: `cargo test -p mackesd workers::datacenter_orchestrator::tests::hostile_provider_process_cannot_pin_the_datacenter_worker_group -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live provider failure/recovery proof.
