# WL-ARCH-009 / WL-FUNC-021 — adfilter Bus recovery (r31)

Date: 2026-08-09

Production source: `crates/mesh/mackesd/src/workers/adfilter.rs`

Source SHA-256:
`2b3c2d61ffaab80ceb59056e175868b9d5bfaf7526468e593d01c8a0816afff3`

## Correction and state semantics

`AdfilterWorker` no longer terminates successfully when the Bus root cannot be
resolved or opened during startup. An explicit root remains exact; otherwise
normal mde-bus resolution falls back to the canonical
`mde_bus::SYSTEM_BUS_ROOT`. The same worker retries unresolved roots, failed
opens, and failed activation reads with shutdown-aware exponential backoff
bounded from 10 ms to 2 s.

`action/adfilter/{allow,block}` contains transient privileged mutations. All
existing action topics and tails are discovered into a candidate cursor map and
installed atomically only after every tail read succeeds, so retained allow or
block commands cannot replay after restart or a late Bus. A topic created after
activation is absent from that snapshot and drains from `None`, preserving its
first forward message.

The node-local policy store and Syncthing policy stores are durable state. The
worker restores its local store before waiting for Bus activation and folds it
normally once activation succeeds; durable policy is not discarded or
tail-primed as if it were a command. Runtime action reads are also staged as one
complete sweep. Any topic-list or message-read error leaves cursors and policy
unchanged and skips that tick's persistence, convergence, compile, and status
publication instead of treating unavailable Bus state as empty.

## Focused farm proof

Host: XEN-BIGBOY (`172.20.0.130`)

Slot: `adfilter-bus-r31`

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=adfilter-bus-r31 \
  ./install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::adfilter::tests::late_bus_recovers_without_replay_and_defers_failed_reads \
  -- --exact --nocapture
```

Final post-format rerun: `1 passed; 0 failed; 4,457 filtered out`. The same worker survived an
unresolved root, an unopenable root, and a failed atomic activation read;
restored durable local policy; skipped a retained block mutation; deferred a
new mutation and all convergence/status effects while reads failed; then
processed exactly the first message on the newly appearing forward topic after
reads recovered.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=adfilter-bus-r31 \
  ./install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::adfilter::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,455 filtered out`.

The final source passed farm `rustfmt --edition 2021 --check`, a farm-scoped
`git diff --no-index --check`, and local scoped `git diff --check`. Existing
crate warnings were unrelated to this slice. No broad suite, package build,
network fetch, live Browser VM, or filler test was run.

## Blockers

None for this focused correction. Live Browser-VM policy consumption was
outside this worker startup/recovery slice.
