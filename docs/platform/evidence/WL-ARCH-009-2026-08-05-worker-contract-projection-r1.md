# WL-ARCH-009 — daemon WorkerSpec contract projection (2026-08-05)

The daemon registry now exposes a deterministic `WorkerContract` projection for
the shared `mackes-mesh-types::worker_runtime` model. It maps the registry's
group, role applicability, cadence, queue/cache limits, restart policy, resource
budget, and cleanup ownership into bounded neutral data. Registry rows with
disabled queues, unsupported overflow, hostile identities/config keys, invalid
rank or cleanup bounds, or unadmitted ownership fail closed. The projection does
not invent runtime states, topics, dependencies, actions, or live health.

## Verification

- BigBoy `.130`, slot `wl-arch009-worker-role-r1`:
  `cargo test -p mackesd worker_contract_projection -- --nocapture`.
- Result: `2 passed; 0 failed; 4406 filtered out`.
- Farm-host file-only Rust formatting and `git diff --check` passed.
- Daemon publication/use, six-process supervision, runtime snapshots, and live
  fleet proof remain open; this is a registry-to-contract slice only.
