# WL-FUNC-023 — enroll wizard Status uses the grouped plane — r1

Date: 2026-08-28
Classification: source glue; **not** live-seat and **not** dest
`production_admitted: false`

Skip Construct Health Fix. Dest, join token, WAN IP, and bearer contents
were not invented. `Restart mackesd` was not confirmed.

## Defect

`magic-setup` Status and post-Create/Join self-test iterated
`wizard_services` → `units_for_role`. That enable/mask catalog still lists
monolithic `mackesd.service` on every rank, plus the first-boot oneshot
and a workstation `etcd.service` member. S17 requires the live plane:
grouped `mackesd-*.service` when the RPM unit file is present.

Add-peer failure copy also said "founded lighthouse"; any enrolled node
mints a join token.

`setup_action.rs` was out of this slice (parent-owned unit-count pin).

## Change

Glue over `mackesd_core::onboard::firstboot::runtime_expected_units`:

- `crates/mesh/mde-enroll/src/wizard_status.rs` — `status_units(role,
  grouped_plane)` plus hostile catalog checks.
- `crates/mesh/mde-enroll/src/bin/magic-setup.rs` — Status/self-test walk
  that catalog; add-peer failure copy is enrollment, not lighthouse-only.
- `crates/mesh/mde-enroll/src/lib.rs` — export the module.

## Farm

`.170` slot 1 was cargo-locked on the artifact directory; this run used
slot 2.

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mde-enroll -- --test-threads=1
```

Exit 0 (~10m). Lib: `53 passed, 0 failed` including
`wizard_status::tests::grouped_status_catalog_refuses_monolithic_mackesd_as_the_only_plane`.
`magic-setup` bin: `3 passed, 0 failed` including
`add_peer_failure_copy_is_not_lighthouse_only`. `mde-enroll` bin: 0 tests.

This is source evidence only. It does not close `WL-FUNC-023`, flip
`production_admitted`, or lift `WL-TEST-003`.
