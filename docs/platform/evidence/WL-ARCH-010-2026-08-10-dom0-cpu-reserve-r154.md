# WL-ARCH-010 — Dom0 CPU reserve admission (r154)

Date: 2026-08-10

VM definition validation now rejects zero CPU/memory resources and any
definition that does not leave the reserved Dom0 CPU lane available. The
compute reconciler turns that invalid definition into a permanent failure
before backend effects.

## Farm proof

BigBoy (`172.20.0.130`), slot `arch010-dom0-reserve-r154`:

```text
cargo test -p mackesd --lib workers::workload_vm::tests::definition_refuses_to_overcommit_dom0_cpu_reserve -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4694 filtered out
```

Live Dell/seat-15 capacity acceptance remains open.
