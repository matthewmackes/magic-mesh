# WL-FUNC-021 — boot-readiness startup phase audit (2026-08-07)

## Audit

`boot_readiness` runs on every role-0 node. Its source probe batch can invoke
`systemctl`, read the Nebula overlay address, inspect the shared substrate and
directory store, and perform an etcd TCP check. Its supplementary batch can
open the directory, fork up to 24 bounded `ping` processes, and probe local
services. These are synchronous operations and are therefore dispatched on a
blocking task.

The existing implementation already contains the appropriate steady-state
containment: fabric, ping, and service groups retain their last observation;
failed groups retry at bounded 4/8/16/32/60-second intervals; healthy groups
recheck after 10 seconds; and the two-second publication heartbeat is retained
for readiness consumers. That heartbeat was intentionally not coalesced in
this audit because its timestamp/freshness behavior is part of the boot-status
contract.

The remaining common-mode risk was startup: every seat entered the first full
blocking batch immediately after the worker started, then anchored its recurring
two-second loop to that same startup boundary. A fleet restart could therefore
fork the same helper processes and issue the same filesystem/network probes in
lockstep.

## Change

`crates/mesh/mackesd/src/workers/boot_readiness.rs` now derives a stable FNV-1a
phase from the node identity. The first batch waits for the existing interval
minus that phase, with a maximum phase window of 1.5 seconds. Consequently,
the first probe remains due within the original two-second freshness deadline,
while identified seats normally start at different deterministic offsets.
The delay is shutdown-aware; an empty identity retains a zero hash phase and
does not introduce identity-dependent behavior. Subsequent ticks retain their
existing cadence and all prior retry/cache semantics.

## Verification

The first focused lane was attempted on `.90`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=boot-readiness-phase-r1 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
workers::boot_readiness::tests:: --features async-services --locked -- --nocapture
```

It stopped during the cold dependency build with `No space left on device`
before compiling the worker. The same disposable source snapshot was routed to
BigBoy with incremental artifacts disabled:

```text
CARGO_INCREMENTAL=0 MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=boot-readiness-phase-r2 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
workers::boot_readiness::tests:: --features async-services --locked -- --nocapture
```

Result: **11 passed, 0 failed; 4,396 filtered out**. This includes the new
`initial_phase_is_stable_bounded_and_preserves_probe_deadline` regression and
the existing backoff/cache classification tests. The farm build emitted
pre-existing warnings in unrelated modules; no error was reported for the
changed worker.

No live seat was restarted or installed in this scoped audit, so post-install
CPU reduction remains a separate runtime acceptance gate.
