# WL-FUNC-019 mDNS name-collision isolation — 2026-08-11

- Scope: unauthenticated LAN discovery metadata cannot enrich an authenticated mesh-peer resource identity.
- Hostile boundary: an mDNS instance-name collision at a different address remains a separate provenance-preserving, approval-gated card and cannot inject its transport into the peer card.
- Focused gate: `cargo test -p mackesd workers::desktop_sources::tests::mdns_instance_collision_cannot_fabricate_a_mesh_peer_transport -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 13,494,640 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,870 filtered out.
- Remaining boundary: live mDNS expiry/recovery and production browser/connect-action separation proof remain.
