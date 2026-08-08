# WL-ARCH-010 bounded Workload contract duplicate-key admission — 2026-08-06

## Boundary implemented

`crates/mesh/mackes-mesh-types/src/workloads.rs` now scans the complete JSON
value recursively before decoding `WorkloadOperationRequest`. Object keys must
be unique at every nesting level, so a duplicate top-level `schema_version` or
nested resource field cannot be silently accepted by last-key-wins parsing.
The existing payload, schema, identifier, resource, deadline, and transition
validation still runs after the duplicate-key guard.

## Verification

- Farm `.50`, slot `workload-contract-duplicate-20260806-r1`:
  `cargo test -p mackes-mesh-types workloads::tests:: -- --nocapture` passed
  **9/9** after correcting scalar visitor handling.
- The hostile test covers duplicate `schema_version` and nested `resources.vcpu`.
- Local `git diff --check` passed. The local host has no `rustfmt` binary;
  formatting was therefore left to the farm cargo gate and no broad rewrite
  was attempted.
- Source SHA-256:
  `5f85497068489fdbbf032d9be681ead6d4e77abd204b82a06cf6ec28e168d3c7`.

## Remaining gap

This proves the bounded request wire boundary only. Reconciler restart/CAS
recovery, live HostCapacity admission, real libvirt/Quadlet adapters, native
Display1/KMS attachment, packaging, and Dell/seat acceptance remain open.
Dell runtime was not modified.
