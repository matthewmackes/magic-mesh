# WL-ARCH-010 / WL-ARCH-009 — KVM health Bus recovery r73

Date: 2026-08-09

## Outcome

`KvmHealthWorker` no longer captures one optional `Persist` handle at process
startup. Production publication resolves the current Bus root and opens a fresh
transaction for every tick, so an unavailable startup root and a same-path
`index.sqlite` replacement recover without restarting `mackesd`. Explicit
`with_bus_root(None)` remains an intentional test/offline disable.

Publication errors are returned to the run loop and retried on the bounded poll
cadence. A failed open or write therefore cannot terminate the worker or be
reported as a successful KVM health publication.

## Focused farm verification

Host: machine 196 (`172.20.0.196`)

Slot: `kvm-health-bus-r73`

- `workers::kvm_health::tests::worker_recovers_late_and_replaced_bus_without_restart`: PASS — 1 passed, 0 failed, 4562 filtered out.
- `workers::kvm_health::tests::tick_publishes_cli_equivalent_row_in_process`: PASS — 1 passed, 0 failed, 4562 filtered out.
- Farm `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/kvm_health.rs`: PASS.
- Scoped `git diff --check`: PASS.

Source SHA-256:

```text
f3c2d720c37d05ffac5ecb69f24edbb21aa3c1707eddd392d6115d5e0a1c3ada  crates/mesh/mackesd/src/workers/kvm_health.rs
```

## Residual boundary

These focused fixtures prove recovery and projection equivalence. They do not
claim live nested-KVM, guest-readiness, or seat-15 presentation proof.
