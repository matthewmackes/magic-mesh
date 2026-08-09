# WL-ARCH-009 compute Bus recovery r12 — 2026-08-09

`ComputeExposeWorker` no longer returns successfully when `Persist::open`
temporarily fails. An absent configured/user root selects the documented
shared `/run/mde-bus` system spool; unavailable storage retries on a fixed,
shutdown-aware cadence clamped to 10–400 ms, then continues through the
existing startup firewalld seed before normal action polling.

The absence of `firewall-cmd` remains a static unavailable-provider condition.
That branch performs no Bus resolution or open probes and waits only for
shutdown instead of returning and inviting supervisor restart churn. A small
file-local runtime seam makes provider presence, root resolution, and Bus open
outcomes deterministic in tests; production still uses the existing PATH
probe, configured/default Bus resolver with the canonical system fallback, and
`Persist::open`.

The recovery loop is entirely before WAN-zone detection, the firewalld startup
seed, durable action-journal handling, and polling. Their established ordering
is therefore preserved.

## Focused farm verification

Machine 194 (`172.20.0.170`), slot `compute-bus-recovery-r12`:

```text
cargo test -p mackesd --features async-services --lib \
  workers::compute_expose::tests::missing_firewall_provider_quiesces_until_prompt_shutdown \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,412 filtered out**. The test proves the static
provider branch stays alive, performs zero Bus probes, materializes no root,
and responds promptly to shutdown.

```text
cargo test -p mackesd --features async-services --lib \
  workers::compute_expose::tests::unresolved_bus_root_retries_without_early_exit_and_stops_promptly \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,412 filtered out**. The test proves repeated
resolution attempts without early worker exit or open attempts, followed by
prompt shutdown while waiting between retries.

```text
cargo test -p mackesd --features async-services --lib \
  workers::compute_expose::tests::transient_bus_resolution_and_open_failure_recovers_forward_without_restart \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,412 filtered out**. A single worker survives one
unresolved-root result and one injected open failure, then consumes a signed
Mesh expose action, performs exactly one rich-rule add plus one reload, and
publishes the exact ULID-correlated action result.

```text
cargo test -p mackesd --features async-services --lib \
  workers::compute_expose::tests::default_bus_root_uses_the_shared_mde_bus_resolver \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,413 filtered out**. Explicit roots are
preserved and the absent-user-root branch selects the canonical system spool.

Exact remote formatting check also passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/compute_expose.rs
```

Base commit: `f14d3a0c08c8a05b17c04dc00d639f9e84d0b4e6`.

Source SHA-256:
`396a35376843e907f0fc9cfb518a6aef7851477d8706d03a3624fce77cccde24`.

## Scope

No broad crate tests, package build, installed-seat firewalld proof, or live Bus
mount-race proof was run. This checkpoint is limited to the worker's startup
availability and corrected-forward transition boundaries.
