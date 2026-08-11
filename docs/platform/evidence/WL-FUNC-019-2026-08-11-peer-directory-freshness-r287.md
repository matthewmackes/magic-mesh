# WL-FUNC-019 peer-directory freshness evidence — 2026-08-11

- Scope: downstream Workload, App, and Android resource projection now accepts
  a remote publisher only from a current, available peer-directory identity.
  The local publisher remains independently authorized.
- Hostile boundary: expired, future-dated, and explicitly unavailable peer rows
  cannot lend identity authority to otherwise plausible downstream resources.
- Focused gate: `cargo test -p mackesd --lib --features async-services workers::service_aggregator::resource_adapters::tests::stale_or_unavailable_peer_directory_cannot_authorize_downstream_resource_reads -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,831 filtered out.
- Remaining boundary: universal live discovery, authenticated remote actions,
  presentation, and release recovery acceptance remain open.
