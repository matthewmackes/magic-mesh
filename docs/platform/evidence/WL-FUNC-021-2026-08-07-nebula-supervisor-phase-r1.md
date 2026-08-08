# WL-FUNC-021 — Nebula supervisor phase audit (2026-08-07)

`NebulaSupervisor` performed its config repair, leadership lookup, role
transition, overlay-IP publication, and lighthouse-roster reconciliation on a
shared five-second startup boundary. It now applies a deterministic node-id
phase capped at 1,500 ms before the first full sweep, while preserving the
existing tick body and retry cadence. Shutdown is selected during the phase;
the first sweep remains within the existing freshness window.

Farm `.90`, slot `nebula-supervisor-phase-r1`:

```text
cargo test -p mackesd nebula_supervisor --features async-services --locked -- --nocapture
test result: ok. 57 passed; 0 failed; 4347 filtered out
```

The focused set includes the new stable/bounded/identity-scoped phase test and
the existing config, identity-rotation, leadership, roster, security, retry,
and shutdown coverage. This is source/farm evidence; live-seat CPU impact and
Dell deployment remain unverified.
