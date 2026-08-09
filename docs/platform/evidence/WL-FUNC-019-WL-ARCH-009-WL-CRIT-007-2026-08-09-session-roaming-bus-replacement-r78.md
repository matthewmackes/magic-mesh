# WL-FUNC-019 / WL-ARCH-009 / WL-CRIT-007 — Session Roaming Bus replacement r78

Date: 2026-08-09

## Outcome

`SessionRoamingWorker` now opens a fresh Bus transaction on every cadence and
binds each read to the current `index.sqlite` device/inode. Initial activation
still rebuilds the complete signed policy fold after a late Bus becomes
available. A same-path replacement preserves that folded policy/layout view,
atomically tail-primes the replacement action lane, skips its retained
transient commands, and processes the first forward command without daemon
restart.

Rows are fully read and the index identity is rechecked before authorization
can mutate its replay ledger or convergence can touch the shared session/layout
planes. An unavailable, unreadable, or repeatedly replaced Bus therefore
retains the prior cursor/fold and defers all convergence instead of treating
the policy log as empty.

## Focused farm verification

Host: machine 196 (`172.20.0.196`)

Slot: `session-roaming-bus-r78`

- `same_path_bus_replacement_skips_retained_policy_and_runs_forward_policy_once`: PASS — 1 passed, 0 failed, 4570 filtered out.
- `bus_open_retry_preserves_state_then_processes_queued_policy_once`: PASS — 1 passed, 0 failed, 4570 filtered out.
- Farm `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/session_roaming.rs`: PASS.
- Scoped `git diff --check`: PASS.

Source SHA-256:

```text
e66cad0aa8e019ba072242386952ceda8fe2beca80ac2e55e23546d619530194  crates/mesh/mackesd/src/workers/session_roaming.rs
```

## Residual boundary

The shared `SessionStore` and `LayoutStore` remain separate convergence
authorities with their existing idempotent/best-effort semantics. These tests
prove Bus recovery and exact policy admission; they do not claim a rendered
two-seat roaming session on live hardware.
