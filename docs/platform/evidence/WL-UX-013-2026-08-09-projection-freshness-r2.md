# WL-UX-013 projection freshness checkpoint (r2)

Date: 2026-08-09

The canonical System and Mesh Health roster fold no longer turns a fold into
new evidence. Its aggregate freshness now ends at the earliest admitted node
publication expiry, and a source-free fold cannot exceed the ten-minute health
publication contract even when a caller supplies an oversized validity value.

Production and focused regression source:

- `crates/mesh/mackes-mesh-types/src/health.rs`
- SHA-256: `657f3a0d907fc5ad245a0f486bad2a9c486341e1caef8cca6417a4152a34c5df`

Farm verification used host `172.20.0.90`, slot
`ux013-health-projection-freshness-r1-20260809`:

- `cargo test -p mackes-mesh-types health::tests::`: 14 passed, 0 failed.
- Direct changed-file `rustfmt --edition 2021 --check`: passed.
- Changed-file `git diff --check`: passed.

The hostile regression supplies `u64::MAX` validity and proves both that an
admitted source's earlier expiry wins and that an empty fold remains bounded.
`docs/platform/WORKLIST.md` was not edited.
