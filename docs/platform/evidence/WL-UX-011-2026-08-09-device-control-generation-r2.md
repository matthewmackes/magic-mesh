# WL-UX-011 device-control generation admission — 2026-08-09

Scope: bind each hardware mutation to the exact node-provider inventory snapshot
the operator inspected. The requester writes `expected_inventory_published_at_ms`;
the node admits only a nonzero exact match before planning or executing a control.
Superseded requests fail closed, enter the existing hash-chain audit history, and
cannot reach the sysfs mutation seam. This is distinct from the previously landed
exact host/category/name/sysfs/driver ownership check.

Farm: `172.20.0.90`, slot `ux011-device-generation-r1-20260809`.

- `cargo test -p mackes-mesh-types device_control -- --nocapture`: PASS, 6 passed,
  0 failed, 480 filtered out.
- `cargo test -p mackesd --lib --features async-services
  workers::device_control::tests -- --nocapture`: PASS, 17 passed, 0 failed,
  4,337 filtered out. This includes
  `superseded_provider_generation_cannot_reach_the_mutation_seam`.
- Independent `cargo test -p mde-shell-egui
  device_manager::tests::dispatch_to_a_fresh_host_writes_the_request_to_the_targets_replicated_dir
  -- --exact --nocapture`: PASS, 1 passed, 0 failed, 1,486 filtered out.
- Exact-file Rust 1.94 rustfmt check on `.90`: PASS.
- `git diff --check`: PASS.

Source SHA-256:

- `mackes-mesh-types/src/device_control.rs`: `5f005f7c15a7d985237ba7d7f769966350f8d1e0fa1d121e40940db59d20352a`
- `mackesd/src/workers/device_control.rs`: `14725703f1e28b444bba10a45f0dd66c4ecb1f7252658d043cf24a78439ff1c0`
- `mde-shell-egui/src/device_manager/mod.rs`: `06f246270bf2679b7eccf3f3d4eed82ce8d4fe98e5058aefa637b7e4392d12cb`
- `mde-shell-egui/src/device_manager/tests.rs`: `8054682fe9d5aa12b1fe615f731daa94ea5a59f788bbcbc38d4a087d149880e9`

Production/live hardware was not mutated; the focused executor fixture uses a
temporary sysfs-shaped control file and proves that a superseded request leaves
that control unchanged while recording the refusal in hash-chained audit history.
