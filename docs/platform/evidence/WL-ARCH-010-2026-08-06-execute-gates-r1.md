# WL-ARCH-010 — current Execute/Workload farm gates (2026-08-06)

Status: verification slice complete; the parent epic remains `Remaining`
because live provider, Workload, Display1/KMS, Dell, and seat acceptance proofs
are still open.

## Scope

This slice reran the current governed Execute paths already present in the
working tree. It adds no second Bus, Workload, executor, or command-launch
authority. The Workload lane covers the sole typed executor recovery path; the
Datacenter lane covers the sole `action/dc/*` responder path.

## Farm verification

All heavy test work used explicit farm hosts and isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=execute-drain-workload-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd workload_compute
result: 22 passed, 0 failed; 4,388 filtered out

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=execute-drain-datacenter-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd datacenter
result: 120 passed, 0 failed; 4,290 filtered out
```

The Workload tests covered durable replay and journal-before-side-effect,
queued and running cancellation, retry/backoff recovery, capacity and role
fail-closed admission, lighthouse rejection, Display1 lease/runtime cleanup,
typed replies, and bounded action recovery. The Datacenter tests covered the
typed VM/storage/genesis/lighthouse responder contracts, authorization and
replay refusal, operation locks, input validation, resource generation, and
the hostile 65-request bounded action-recovery case.

The farm builds emitted existing unused/dead-code and documentation warnings;
no warning was promoted to a failure. The exact scratch workspaces were
removed after completion. No BigBoy result is claimed for this rerun; the
earlier BigBoy Datacenter and Workload evidence remains separately recorded.

## Remaining proof

These fixture-backed gates do not prove live libvirt/XAPI/Quadlet execution,
restart/crash recovery against live providers, Display1/KMS scanout, caller
migration, Dell/seat-15 acceptance, or full RPM promotion. Those remain
`Remaining` under WL-ARCH-010 and the active drain goal.

## Source hashes at capture

```text
0f4d5f2965c0d8b901681028107d4227f9cde5a3d23969e73734a0f56c2af9f1  crates/mesh/mackesd/src/workers/workload_compute.rs
a51017676cf3d56b1356544e81f6d6c05651804590cdfb8eecc3ecc8f03ab5c4  crates/mesh/mackesd/src/ipc/datacenter.rs
```

Working-tree base revision: `e52322ec` (changes are intentionally
uncommitted).
