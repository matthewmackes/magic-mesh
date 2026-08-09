# WL-ARCH-010 / WL-FUNC-019 — mesh-mount Bus startup recovery (r29)

Date: 2026-08-09

## Scope

The `mesh_mount` worker now keeps the same worker invocation alive when its Bus
root is unresolved, unopenable, or not yet safe to activate. Production root
resolution preserves an explicit override and otherwise falls back to the
canonical `mde_bus::SYSTEM_BUS_ROOT`. Startup retries use shutdown-interruptible
exponential backoff bounded from 10 ms through 2 s.

Every `action/mesh-mount/<host>` input is a transient, privileged lifecycle
action (`mount`, `escalate`, or `unmount`); this worker has no durable state-fold
input lane. Activation discovers all existing action topics and reads every tail
into a candidate map before installing any cursor. Retained destructive effects
therefore never replay, including after a failed partial activation. A per-host
topic absent from that successful startup snapshot is forward work and drains its
first message from `list_since(None)` before advancing normally.

Runtime polling also stages all topic reads and cursor advances before executing
any action. A `list_topics` or `list_since` error aborts the sweep without a
partial cursor commit and defers mount, unmount, idle, probe, and reconnect
effects instead of treating the unread Bus as empty state.

## Focused farm proof

Host: machine194, `172.20.0.170`

Slot: `mesh-mount-bus-r29`

The final source was synchronized with explicit routing:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=mesh-mount-bus-r29 \
  ./install-helpers/xcp-build.sh sync
```

The primary exact test used the same explicit route:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=mesh-mount-bus-r29 \
  ./install-helpers/xcp-build.sh cargo test -q -p mackesd \
  --features async-services --lib \
  workers::mesh_mount::tests::late_bus_and_new_host_topics_recover_without_replay_or_restart \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,453 filtered out`. The same worker survived an
unresolved root, an open failure, and a fail-closed activation-tail failure. The
startup-retained mount caused zero backend effects; a forward request on the
startup host and the first request on a newly appearing host topic each mounted
exactly once. Shutdown completed without supervisor restart.

Two exact tests reused the warmed routed slot:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::mesh_mount::tests::request_topic_activation_is_atomic_when_a_tail_read_fails \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::mesh_mount::tests::service_bus_root_falls_back_to_the_shared_system_spool \
  -- --exact --nocapture
```

Results: each `1 passed; 0 failed; 4,455 filtered out`. The first proves a
second-topic tail failure installs no candidate cursor; the second proves the
system-spool fallback while preserving explicit roots.

Final single-file farm formatting gate:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/mesh_mount.rs
```

Result: passed.

Scoped local integrity gate:

```text
git diff --check -- crates/mesh/mackesd/src/workers/mesh_mount.rs \
  docs/platform/evidence/WL-ARCH-010-WL-FUNC-019-2026-08-09-mesh-mount-bus-recovery-r29.md
```

Result: passed.

## Artifact identity

```text
868488d05528f71760e8b3507830f4a464da579af4384d57a61cc1ae0bf2e077  crates/mesh/mackesd/src/workers/mesh_mount.rs
```

No WORKLIST edit or commit was made.
