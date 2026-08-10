# WL-ARCH-010 — shared guest CPU-pool evidence

- Date: 2026-08-10
- Farm host: `172.20.0.90`
- Farm slot: `arch010-shared-cpu-pool-r152`
- Gate: `cargo test -p mackesd --lib workers::workload_vm::tests::shared_guest_pool_avoids_colliding_per_vcpu_pins -- --nocapture`
- Result: 1 passed, 0 failed

The domain XML now assigns each admitted VM the shared non-Dom0 CPU pool
(`1..host_threads-1`) and pins emulator/I/O threads to that pool. Per-vCPU pins
were removed because separate VMs could otherwise collide on the same host CPUs.
