# WL-FUNC-021 — boot-readiness failed-probe containment (2026-08-06)

The boot-readiness worker was a common-mode CPU risk: its synchronous
`systemctl`, TCP, filesystem, and `ping` probes ran from the async worker at a
fixed two-second cadence. A missing service or peer could therefore keep
restarting blocking work on every seat even while the published readiness
snapshot remained unchanged.

## Change

The probe batch now runs on `spawn_blocking`, keeps the last honest observation
while a retry is pending, and independently backs off failed fabric, peer-ping,
and service probe groups at 4/8/16/32/60 seconds. A successful group is
rechecked after a bounded 10-second healthy interval rather than every publish
tick; the publication cadence remains two seconds so consumers still see fresh
timestamps and cached status. Shutdown remains a responsive select arm.

Implementation: `crates/mesh/mackesd/src/workers/boot_readiness.rs`.

## Verification

The first `.90` attempt was stopped by the farm's documented `/home` capacity
limit while linking the broad package test binary (`ENOSPC`); that exact
disposable slot was removed. The focused library gate was then rerouted to
BigBoy `.130`, slot `boot-readiness-cpu-r2`, after an explicit sync with
`CARGO_INCREMENTAL=0`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=boot-readiness-cpu-r2 \
./install-helpers/xcp-build.sh sync
ssh mm@172.20.0.130 'cd /home/mm/magic-mesh-farm-boot-readiness-cpu-r2 && \
CARGO_INCREMENTAL=0 cargo test -p mackesd --lib \
workers::boot_readiness::tests:: --features async-services --locked -- --nocapture'
```

Result: **10 passed, 0 failed**; 4,383 library tests filtered. The source also
compiled successfully in the earlier farm `mackesd` media-registry test build,
with no new code warnings after the documentation fix.

The healthy-recheck change was independently verified on `.90`, slot
`boot-readiness-healthy-cadence-r1`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=boot-readiness-healthy-cadence-r1 \
./install-helpers/xcp-build.sh cargo test -p mackesd --lib \
workers::boot_readiness::tests:: --features async-services --locked -- --nocapture
```

Result: **10 passed, 0 failed**; 4,383 library tests filtered.

No live seat was installed or restarted. Post-install CPU reduction remains an
explicit acceptance gate.
