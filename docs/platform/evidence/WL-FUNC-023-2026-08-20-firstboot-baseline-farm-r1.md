# WL-FUNC-023 S17 first-boot baseline farm evidence — 2026-08-20

- Source worktree: dirty `agent/drain-worklist-20260725` at
  `41080a75c822a019252a06778f1474f7751532c1` plus the S17 first-boot auditor
  (`crates/mesh/mackesd/src/onboard/firstboot.rs` and callers).
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `1`
- Command:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib firstboot -- --nocapture
```

- Result: `5 passed, 0 failed, 5023 filtered out` in 4m 16s.

| Test | Result |
|---|---|
| `planted_missing_unit_cannot_produce_ready` | ok |
| `compute_and_hardware_failures_are_warnings` | ok |
| `healthy_baseline_stamps_converged_and_keeps_tokens` | ok |
| `unit_file_does_not_use_status_or_unconditional_touch` | ok |
| `firstboot_resume_keeps_pending_capsules` | ok |

The unit no longer runs `mackesd status` or unconditionally touches
`firstboot-converged`. Core failures refuse the marker; capability (KVM /
hardware) failures stay warnings; pending commissioning capsules survive a
failed audit. Related CLI parse coverage was refused on `.170` (`/home` below
the 8 GiB admission floor) and rerouted to `.196`.

## Role-provision follow-up — 2026-08-20 `.50`

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib onboard::role_provision -- --nocapture
```

- Result: `24 passed, 0 failed, 5004 filtered out` in 5m 23s.
- Includes first-boot unit shipping/enablement and rank-0 catalog membership.

## Self-test follow-up — 2026-08-20 BigBoy slot 2

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib onboard::self_test -- --nocapture
```

- Result: `29 passed, 0 failed, 4999 filtered out`.
- Role-daemon expected counts still come from the reused role-provision catalog.

## Lifecycle-authority follow-up — 2026-08-20 `.90`

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib lifecycle_authority -- --nocapture
```

- Result: `17 passed, 0 failed, 5011 filtered out` in 8m 21s.
- `replace_checks` did not regress exclusive locks, capsule retry, or offboarding receipts.

This is product-core package/first-boot evidence only; exact installed-seat
acceptance remains under `WL-TEST-002`.
