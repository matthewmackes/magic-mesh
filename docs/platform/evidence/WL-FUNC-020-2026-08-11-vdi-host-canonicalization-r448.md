# WL-FUNC-020 VDI host canonicalization — 2026-08-11

- Scope: Android VDI provider hosts require canonical lowercase DNS labels with bounded label lengths and valid hyphen placement.
- Hostile boundary: aliases, uppercase spellings, malformed labels, and overlong labels cannot cross the mesh-host authority boundary.
- Focused gate: `cargo test -p mackes-mesh-types android_provider::tests::hostile_vdi_host_alias_cannot_cross_the_canonical_mesh_authority_boundary -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 521 filtered out.
- Remaining boundary: reject the same host substitution through a live Cuttlefish VDI publication and shell attach.
