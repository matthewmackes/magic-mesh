# WL-ARCH-009 S1 registry census r9 — 2026-08-09

## Result

The canonical bidirectional census is exact: every reachable literal tiered,
direct-supervisor, and responder start has one `WORKER_REGISTRY` row, every such
row has a production start, and the sole dynamic `lighthouse_probe` binding
remains explicit. The registry contains 143 rows. Its complete `WorkerSpec`
inventory SHA-256 is
`983c9334b4531f55afb42ea732438ed4cfdc12f0526affbf9b0b3971317ea616`;
hostile cadence or cleanup drift changes the digest.

The highest-impact contradiction was `ansible-pull`: production required
`MDE_ANSIBLE_PULL_URL` and runs every 900 seconds, while the registry declared a
runtime inventory dependency and inherited on-demand cadence. The row now owns
the exact environment predicate and 900-second cadence. `spawn_tiered` enforces
registry-owned startup environment predicates before construction, and the
parallel hand-written ansible gate was removed. Unknown workers, empty required
values, and explicit false values fail closed.

The next incomplete ownership edge is output topology: neutral worker contracts
still intentionally leave publications, subscriptions, dependencies, and action
descriptors empty. This audit did not invent endpoints or add another inventory.

## BigBoy verification

- Host `172.20.0.130`, slot `arch009-registry-r9`.
- Initial focused worker-role audit: 27 passed; the sole failure printed the
  bootstrap digest above for deliberate pinning.
- Exact inventory-hash regression: 1 passed, 0 failed.
- Exact startup-configuration hostile regression: 1 passed, 0 failed.
- Exact bidirectional spawn/registry census: 1 passed, 0 failed.
- Touched hunks match rustfmt output; whole-file checking still reports the
  pre-existing navigation/clock layout outside this change.
- Scoped `git diff --check`: passed.

## Source hashes

- `b4f045f95b1b4fbf0c04295056df86bd806b937b4d31ab7712d94fcfce2bc051`
  — `crates/mesh/mackesd/src/worker_role.rs`
- `605468d72cd8231783858db488f1e660d375bcb22c68ed227dd8846b39b3fb91`
  — `crates/mesh/mackesd/src/bin/mackesd/spawn.rs`

WL-ARCH-009 remains open for typed output/dependency/action declarations,
process-group cutover, Workers UI completion, and live fleet isolation proof.
