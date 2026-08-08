# WL-FUNC-021 — five-seat CPU/NWS recovery, release 11 (2026-08-08)

## Finding and correction

Release 10 exposed a startup-order defect on T480. The NWS forecast worker
published its honest no-fix projection before the Bus supervisor initialized
the retained store. Bus startup replaced that early row, while the worker's
same-cause coalescing suppressed every later no-fix publish. The worker was
healthy, but T480 had no `state/overlay/nws-hourly/T480` record.

The NWS worker now suppresses a repeated degraded cause only while the retained
projection still exists. A missing row is repaired on the next phased, bounded
retry. This preserves coalescing and fleet desynchronization while recovering
from Bus initialization or store replacement.

## Farm and package proof

- BigBoy `.130`: focused `nws_forecast_overlay` tests passed 14/14, including
  `same_cause_retry_repairs_a_projection_lost_during_bus_startup`.
- `.50`: the changed Rust source passed file-scoped `rustfmt --check`. The
  workspace-wide format gate remains noisy from unrelated concurrent edits.
- BigBoy's native Fedora 44 `.131` builder produced
  `magic-mesh-12.1.6-11.x86_64.rpm`, 87,268,687 bytes, SHA-256
  `7379f0980cd792c5bbacbb87157335642bc36daf39cbb25dd692a5d776271a7d`.
- The complete payload and 90 MiB size gates passed.

## Five-seat live proof

T480, Eagle, seat 15, Dell, and Surface matched the artifact hash and passed
RPM transaction preflight before installation. All five report release 11,
active current-package `mackesd`, one Music daemon, one shell, global Music
enablement disabled, and zero service restarts. Dell and seat 15 also passed
the reusable Music live-seat verifier, including `rpm -V`.

Every seat now has one retained NWS hourly row under its own hostname. T480's
new row explicitly reports `fresh same-host MG90 fix unavailable`, proving the
lost-startup-row repair rather than fabricating weather data. The other four
seats retain the same honest no-fix state because no fresh MG90 position is
available.

A synchronized release-11 CPU observation sampled current RPM-owned `mackesd`
five times at two-second intervals on all five seats. Every seat held a stable
PID and `NRestarts=0`; maximum and mean CPU were 0 permille of one core, below
the existing 850/500 permille acceptance thresholds.

Dell briefly dropped LAN and overlay connectivity during package activation,
then returned with release 11 active. Its Browser VM remained defined and shut
off. Temporary seat RPM copies were removed and normal BigBoy `.130` capacity
was restored.

## Remaining WL-FUNC-021 boundary

This closes the synchronized five-seat CPU/NWS startup-recovery gate. Natural
provider-loss/recovery continuity, physical DLNA/Chromecast rendering,
cross-seat owner handoff, mutating playback on T480/Eagle/Surface, and human
speaker judgment remain open.
