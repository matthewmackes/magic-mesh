# WL-ARCH-010 — uncommitted attachment revocation (r124)

Date: 2026-08-10

Base revision: `56a3cb17`

Commit: `a5e9fd54`

## Defect and correction

A Workload actuator could return a new Display1 attachment lease before the
ledger had durably admitted the final outcome. The reconciler copied that lease
into synthetic intermediate phases. If a phase transition was rejected, the
capability could remain active without a durable owning status.

The reconciler now attaches a returned lease only at the final outcome commit
point. Any rejected transition immediately revokes the still-uncommitted lease.
Retained request replay remains read-only and cannot repeat the actuator effect
or recreate the rejected capability.

## Focused farm proof

Build VM `.90` (`172.20.0.90`) passed:

```text
cargo test -p mackesd --lib workers::workload_compute::tests::rejected_attachment_outcome_revokes_uncommitted_lease_without_replay_effect -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 4672 filtered out
```

Source SHA-256:

- `20c95ab97b951558d7efb3aa2840bab96e53726dc5dd9a7947606c0c96cbaf7c`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`

Production Display1/libvirt crash injection and installed-release recovery
remain open.
