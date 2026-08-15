# WL-FUNC-011 Files route hard-cut evidence

Date: 2026-08-15

The standalone `Surface::Files` route is now a migration alias only. Surface
normalization opens the canonical Files section inside Communications, the
legacy render arm is unreachable, and workspace canonicalization maps Files to
Communications. This removes the duplicate top-level route without changing
the canonical Files implementation.

Farm verification on BigBoy (`172.20.0.130`):

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=collab-files-hardcut-farm30 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-shell-egui legacy_node_surfaces_normalize_into_workers_tabs -- --nocapture
```

Result: `1 passed; 0 failed`.
