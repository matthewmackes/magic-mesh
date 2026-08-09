# WL-ARCH-009 CUPS-sync Bus recovery r46 — 2026-08-09

## Semantic result

`cups_sync` no longer captures an unresolved user Bus root as a permanent action
disable. The worker resolves an explicit/user root at startup, falls back to
`mde_bus::SYSTEM_BUS_ROOT`, and keeps retrying a late or unopenable Bus in the
same supervised process with shutdown-aware exponential backoff bounded from
10 ms through 2 s. Each action attempt fresh-opens the Bus, so external writers
and a replaced index are observed. A statically absent optional CUPS stack has
no periodic CUPS convergence timer or subprocess/filesystem effect.

Both fixed transient command lanes, `action/printers/sync-now` and
`action/printers/list`, are fully read to their tails before activation installs
any cursor. Retained commands are therefore skipped atomically after restart,
while a lane created after activation admits its first command. Every runtime
action sweep stages complete reads of both lanes before dispatching any action;
one failed lane read leaves every cursor and effect untouched.

Reply publication is now the cursor commit boundary. A completed action whose
required reply write fails is retained in a bounded 64-entry in-memory pending
reply map and retried without repeating the sync effect in the same worker
process. This is not a durable outbox: a daemon/process crash after the CUPS or
filesystem effect completes but before reply publication can lose the barrier
and repeat the destructive/configuration effect after restart. This slice does
not claim that process-crash window is closed.

## Machine-193 verification

All meaningful verification ran on canonical machine 193 address
`172.20.0.90`, with explicit
`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cups-sync-bus-r46`. No local Cargo
command was run.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cups-sync-bus-r46 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-cups-sync-bus-r46 && cargo test -q -p mackesd --features async-services --lib workers::cups_sync::tests::late_bus_atomic_tail_prime_and_forward_sync_once -- --exact --nocapture'
# PASS: 1 passed; 0 failed; 4491 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cups-sync-bus-r46 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-cups-sync-bus-r46 && cargo test -q -p mackesd --features async-services --lib workers::cups_sync::tests::final_lane_read_and_reply_failure_are_fail_closed -- --exact --nocapture'
# PASS: 1 passed; 0 failed; 4491 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cups-sync-bus-r46 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-cups-sync-bus-r46 && cargo test -q -p mackesd --features async-services --lib workers::cups_sync::tests::dynamic_first_command_system_fallback_and_retry_shutdown -- --exact --nocapture'
# PASS: 1 passed; 0 failed; 4491 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cups-sync-bus-r46 ssh -i /root/.ssh/mackes_mesh_ed25519 -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 mm@172.20.0.90 'cd ~/magic-mesh-farm-cups-sync-bus-r46 && rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cups_sync.rs'
# PASS

git diff --check -- crates/mesh/mackesd/src/workers/cups_sync.rs docs/platform/evidence/WL-ARCH-009-2026-08-09-cups-sync-bus-recovery-r46.md
# PASS
```

## Source hash

```text
f5c72a95a4af0061bbbdeb53971eb3c630a160f478a03af01e1854b8563bd776  crates/mesh/mackesd/src/workers/cups_sync.rs
```

The local and machine-193 hashes matched. `WORKLIST.md` was not edited, and no
commit or push was made.
