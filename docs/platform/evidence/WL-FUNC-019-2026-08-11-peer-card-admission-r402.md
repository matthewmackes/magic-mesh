# WL-FUNC-019 peer-card admission authority — 2026-08-11

- Scope: downstream Workload, App, and Android adapters may trust only a peer row that itself produces a valid admitted resource card.
- Hostile boundary: malformed peer role metadata cannot authorize downstream reads even when hostname and heartbeat fields appear current.
- Focused gate: `cargo test -p mackesd workers::service_aggregator::resource_adapters::tests::malformed_peer_projection_cannot_authorize_downstream_resource_reads -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 3, admitted with 18,031,160 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,867 filtered out.
- Remaining boundary: physical peer-directory publication through downstream adapters and installed Remote Sessions UI proof remain.
