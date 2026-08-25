# WL-FUNC-023 leftover live-seat — firstboot host-cert paths (2026-08-25)

First-boot `gather_live_in` treated mesh identity as only
`/etc/nebula/host.crt`. Dest-cut seats may hold
`/etc/nebula/identity/current/host.crt` (or neither). First-boot now
uses the same two paths telemetry and nebula_supervisor already use.

This is source-path coverage only. It does **not** close live-seat
overlay-ip leftover work.

## Change

`crates/mesh/mackesd/src/onboard/firstboot.rs`

- Helper: `mesh_identity_present_under(nebula_root)` — true when either
  `identity/current/host.crt` or `host.crt` is a regular file under the
  injected Nebula root.
- Production `gather_live_in` calls it with `/etc/nebula`.
- Unit test `mesh_identity_present_under_accepts_either_host_cert_layout`
  plants a temp root (no live `/etc/nebula`):
  - neither path → false
  - legacy `host.crt` only → true
  - dest-cut `identity/current/host.crt` only → true

## Farm

- Source worktree: dirty `agent/drain-worklist-20260725` at `4071ed295`
  plus this firstboot helper (uncommitted).
- Farm host: `172.20.0.50` (XEN-HOME-SERVICES)
- Farm slot: `2`
- Command:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib onboard::firstboot
```

- Result: **pass** — `10 passed; 0 failed; 0 ignored; 0 measured; 5068 filtered out` in 0.01s after 5m 58s compile.

| Test | Result |
|---|---|
| `mesh_identity_present_under_accepts_either_host_cert_layout` | ok |
| `planted_missing_unit_cannot_produce_ready` | ok |
| `healthy_baseline_stamps_converged_and_keeps_tokens` | ok |
| `compute_and_hardware_failures_are_warnings` | ok |
| `firstboot_resume_keeps_pending_capsules` | ok |
| `failed_invite_enrollment_cannot_ignore_unit_fail_or_burn_token` | ok |
| `runtime_expected_units_never_require_the_firstboot_oneshot` | ok |
| `runtime_expected_units_use_grouped_plane_and_drop_workstation_etcd` | ok |
| `grouped_mackesd_plane_can_produce_ready_without_monolithic_unit` | ok |
| `unit_file_does_not_use_status_or_unconditional_touch` | ok |

## Remaining blocker

Live dest-cut seats may still have neither cert, so overlay-ip stays
empty and first-boot `mesh_identity` still fails on those seats. This
unit aligns the two paths; it does not enroll identity or publish
overlay-ip. Overlay-ip leftover remains open.
