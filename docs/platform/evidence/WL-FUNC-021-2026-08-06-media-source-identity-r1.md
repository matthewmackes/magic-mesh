# WL-FUNC-021 — Jellyfin upstream identity guard (2026-08-06)

The Media UI projection now drops every ambiguous Jellyfin row when either its
local ID or upstream identity is duplicated, in addition to the existing
endpoint safety boundary. Source SHA-256:

```text
18c5e952dbacdc3b0b6d9606dcf0a1f929fb2aac35bae42d342b30aff1b9dd20  crates/desktop/mde-media-egui/src/model.rs
```

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=media-jellyfin-identity-20260806-r1 install-helpers/xcp-build.sh cargo test --locked -p mde-media-egui --lib mesh_jellyfin_projection_rejects_duplicate_upstream_identity -- --nocapture
```

Result: **1/1 passed**. Live renderer and provider continuity remain open.
