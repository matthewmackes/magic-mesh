# WL-FUNC-021 — Navidrome supervisor startup-phase audit (2026-08-07)

## Finding

`navidrome_supervisor` had no persistent state-file write to suppress. Its
first 30-second pass nevertheless synchronously invoked `systemctl` for the
unit-active and unit-installed checks, so media-capable seats starting one
daemon generation could reach that probe boundary together.

## Change

`NavidromeSupervisor` now reads the local `/etc/hostname` and derives a stable
FNV-1a startup phase bounded to 1.5 seconds. The first probe runs after
`tick - phase`, never later than the previous 30-second deadline; subsequent
30-second cadence and all restart/reprovision decisions are unchanged. The
startup delay is shutdown-aware, and an unavailable hostname preserves the
legacy immediate phase.

## Farm verification

The first focused lane on `.90` reached compilation but stopped with
`No space left on device`; this was an infrastructure failure, not a test
failure. The same real focused gate was rerouted to the warmed BigBoy VM
(`172.20.0.130`) and passed:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=cpu-mitigation-release-r4 \
  install-helpers/xcp-build.sh cargo test -p mackesd navidrome_supervisor \
  --features async-services --locked -- --nocapture
```

Result: **2 passed, 0 failed, 0 ignored; 4,413 filtered out** in
`mackesd_core`, with all other filtered test targets also passing. The new
regression covers deterministic identity mapping, the 1.5-second bound, the
preserved first-pass deadline, the short-interval bound, and the empty-host
fallback; the existing supervisor decision matrix also passed.

## Scope and remaining proof

Changed files for this audit are this evidence record and
`crates/mesh/mackesd/src/workers/navidrome_supervisor.rs`; the active worklist
was not edited. Live multi-seat CPU acceptance still requires reachable Dell
seats and an installed package containing this worker change.
