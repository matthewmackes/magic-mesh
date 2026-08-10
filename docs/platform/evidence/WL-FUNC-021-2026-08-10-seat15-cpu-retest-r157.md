# WL-FUNC-021 — seat-15 CPU retest (r157)

Date: 2026-08-10

## Read-only observation

Seat 15 (`172.20.0.15`, `Basement-Test-Workstation`) was sampled over SSH
without changing services or files. At 14:05 local time it reported load
averages `0.38, 0.57, 0.64`, with one user session and both `mde-musicd` and
`mackesd-compute` active.

The process snapshot was led by Syncthing at `20.8%` CPU and
`mde-shell-egui` at `13.0%`. `mde-musicd` was `3.3%`; `mackesd` group
processes were `3.1%` or lower. The host was not CPU-saturated: load stayed
well below the four-thread capacity and no daemon was near a full host CPU.

## Disposition

This retest does not justify a destructive restart or a new CPU throttle. The
remaining likely contributors are Syncthing convergence and the active shell
diagnostic/session workload. Repeat after sync reaches steady state and with
the diagnostic session closed; only then claim remediation of the user-visible
CPU complaint.
