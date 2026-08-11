# WL-CRIT-007 boot etcd source authority — 2026-08-11

- Scope: configured etcd is the authoritative peer directory for boot-readiness evaluation.
- Hostile boundary: failed etcd reads cannot substitute stale filesystem peers and falsely publish healthy boot readiness.
- Focused gate: `cargo test -p mackesd workers::boot_readiness::tests::configured_etcd_read_failure_cannot_substitute_stale_filesystem_peers -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 10,482,112 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,861 filtered out.
- Remaining boundary: installed reboot/systemd ordering with etcd outage and live lease-backed recovery remains.
