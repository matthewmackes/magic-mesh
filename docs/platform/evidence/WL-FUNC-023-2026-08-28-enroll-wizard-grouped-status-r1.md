# WL-FUNC-023 — wizard Status uses grouped plane; enroll catalog pin (2026-08-28)

Source heal so `magic-setup` Status/self-test matches first-boot's live
plane, and so the wizard unit-count test tracks `units_for_role`.
`production_admitted` unchanged. No live-seat mutation. Dest, token, and
mesh-id were not invented.

## Changes

- `wizard_services` workstation count is 13 (rank-0 plus node-virt extras).
  The old pin of 8 was stale.
- New `wizard_status` catalog: when the grouped RPM unit file is present,
  Status reports `runtime_expected_units` (grouped `mackesd-*.service`, no
  dest-gated collab-identity, no workstation etcd member, no first-boot
  oneshot, no `.timer` leaks). Thin lighthouses without that file still
  report monolithic `mackesd.service`.
- `is_active_argv` takes `&unit` because that catalog returns owned
  `String`s.

## Verification (farm)

First compile on `.170` failed `E0308` (`String` vs `&str`). After the
borrow fix:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-enroll -- --test-threads=1
```

Exit 0. Lib: `53 passed, 0 failed`. Bins: `3 passed`. Pre-existing
`call_media.rs` dead-code warnings are out of this slice.

A later overlapping run on slot 2 (slot 1 was cargo-locked) also exited 0
with the same 53+3 counts, including
`grouped_status_catalog_refuses_monolithic_mackesd_as_the_only_plane`.
See `WL-FUNC-023-2026-08-28-enroll-grouped-status-r1.md`.

This is source evidence only. It does not close `WL-FUNC-023` or lift
`WL-TEST-003`.
