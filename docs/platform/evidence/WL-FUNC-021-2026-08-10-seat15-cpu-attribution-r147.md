# WL-FUNC-021 — seat-15 CPU attribution (r147)

Date: 2026-08-10

## Read-only live result

Seat 15 (`172.20.0.15`, `Basement-Test-Workstation`) was reachable over SSH.
The host reported load averages `1.85, 1.72, 1.67`, 15 GiB RAM with 13 GiB
available, and only 80 KiB of swap in use. This is elevated activity, not a
CPU deadlock or memory-pressure event.

The top sampled processes were Syncthing at 22.6% CPU and `mde-shell-egui` at
19.6%; `mde-musicd` was 3.4% and the grouped `mackesd` services were each at
2.4% or below. Syncthing had restarted 11 minutes earlier and its folder
status showed 12,174 global files, 23,599 total items, 18 items still needed,
and `sync-waiting`. Its system API reported `cpuPercent: 0` on the follow-up
sample, so the Syncthing load is a convergence burst rather than a sustained
runaway loop. The shell service had an attached remote diagnostic shell and
SSH child, which explains part of its live CPU sample.

## Corrective slice

`node_grade` now coalesces semantically unchanged canonical node files, health
Bus rows, and folded snapshots. Node files heartbeat within 60 seconds and
folded snapshots within 15 seconds, below their 30-second freshness bound;
Bus replacement resets the gates. The focused BigBoy `.130` gate passed:
`steady_health_reuses_bus_row_until_bounded_heartbeat` — 1 passed, 0 failed.

## Boundary

No service was stopped, restarted, or throttled. Repeat a bounded seat-15 CPU
sample after Syncthing reaches steady state; live improvement is not claimed
until that observation exists.
