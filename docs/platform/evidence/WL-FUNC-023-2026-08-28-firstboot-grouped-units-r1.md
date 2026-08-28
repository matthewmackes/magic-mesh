# WL-FUNC-023 — first-boot grouped plane skips dest-gated collab-identity (2026-08-28)

Source heal for Seat 15 `lifecycle-firstboot` failing `units`+`verification`
when `mcnf-collaboration-identity.service` is inactive. That unit is Open
Onboarding (dest-gated). First-boot must not invent a collab receipt.
`production_admitted` unchanged. No live-seat mutation.

## Bug

`mackesd onboard lifecycle-firstboot` treated dest-gated
`mcnf-collaboration-identity.service` and workstation `.timer` units as
required grouped-plane members. Seat 15 then stayed on
`pending-convergence` with `missing_requirements: ["units","verification"]`
while the grouped `mackesd-*.service` plane was the real requirement.

## Source change (`crates/mesh/mackesd/src/onboard/firstboot.rs`)

- Filter `mcnf-collaboration-identity.service` (and name leaks) out of
  runtime expected units and `missing_required_units()`.
- Drop `.timer` units on the workstation grouped plane.
- Keep pending enrollment tokens unchanged on a blocking run.

## Verification (farm)

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd firstboot -- --test-threads=1
```

BigBoy `.130` hung compiling (host later unreachable); this result is the `.90` retry, exit 0. Focused lib: `12 passed, 0 failed` including
`grouped_workstation_units_pass_when_collab_identity_is_inactive`. CLI:
`3 passed, 0 failed`. Dead-code warnings in `call_media.rs` are pre-existing
and out of this slice.

This is source evidence only. Installed-seat first-boot after the RPM is
packaged remains a live leftover on `WL-TEST-003` after a testing Beta.
This note does not close `WL-FUNC-023`.
