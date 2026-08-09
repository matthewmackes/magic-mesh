# WL-ARCH-009 Action Console digest binding — 2026-08-09

## Production correction

`WorkerChangeSetRequest` previously admitted any well-formed SHA-256 string without
recomputing it from the staged target, generation, typed items, impact, recovery,
and arming requirement. A structurally valid mutation could therefore retain a
stale digest and reach downstream routing.

Shared contract admission now recomputes and exactly compares the digest before
freshness or daemon routing checks. Digest construction sorts bounded items by
their unique item ID, so daemon canonicalization cannot change an admitted
intent's identity. The hostile regression mutates an action under the retained
digest and proves fail-closed admission.

## Farm proof

- Host: `172.20.0.90`
- Slot: `arch009-action-digest-r1-20260809`
- Exact-file `rustfmt --check`: passed
- `cargo test -p mackes-mesh-types worker_runtime::tests -- --nocapture`: 9 passed, 0 failed
- `cargo test -p mackesd workers::worker_runtime_status::tests --lib -- --nocapture`: 15 passed, 0 failed
- Existing compiler warnings were non-fatal and outside this correction.

## Source identity

- `crates/mesh/mackes-mesh-types/src/worker_runtime.rs`: `5df2c1d734e509a6943c45d5a95444021490a349140c78b3d981fb1dc0ba015d`
- `crates/mesh/mackesd/src/workers/worker_runtime_status.rs`: `3c9ab7fd45503bfcf54cf770186be9154d5b56c5bc97e751f9ada4b698c3eaa4`

No live Action Console round-trip is claimed by this bounded contract slice.
