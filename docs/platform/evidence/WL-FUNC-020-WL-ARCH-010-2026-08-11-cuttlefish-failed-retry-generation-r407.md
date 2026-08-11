# WL-FUNC-020 / WL-ARCH-010 Cuttlefish failed-retry generation — 2026-08-11

- Scope: Cuttlefish guest readiness must bind only to the Workloads generation that currently owns the outer VM.
- Hostile boundary: a failed or cancelled retry retaining the prior VM's `Running` power cannot relabel that old VM with the terminal retry's generation.
- Focused gate: `cargo test -p mackesd workers::cloud::verbs::android::cuttlefish::tests::failed_retry_cannot_rebind_running_outer_vm_to_new_generation -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 14,284,328 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,870 filtered out.
- Remaining boundary: nested-KVM outer-VM replacement with guest readiness, VDI, reconnect, cleanup, and restart proof remains.
