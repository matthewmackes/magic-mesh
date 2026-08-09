# WL-UX-013 recovery target binding — 2026-08-09

## Correction

Health remediation authorization now binds the selected active condition to the
exact requested `HealthScope`, in addition to the existing local-node target,
snapshot-generation, offered-action, and confirmation checks. A condition from
another node can no longer lend its remediation offer to a local mutation.

Production source:
`crates/mesh/mackesd/src/workers/node_grade.rs` (`65f34471a8b38c0d3dfa2d363876e6d69077ac331807d8444c8a1bb27241cae6`).

## Farm proof

Machine 194 build VM `172.20.0.170`, slot
`ux013-recovery-scope-r1-20260809`:

- hostile cross-node authorization regression: 1 passed, 0 failed;
- complete `workers::node_grade::tests` library module: 13 passed, 0 failed;
- exact-file `rustfmt --check`: passed;
- scoped local `git diff --check`: passed.

The first cold attempt exhausted disposable build-target storage. Removing only
farm `target` directories recovered `/home` from 100% used to 30% used (43 GiB
free). A non-library test shape remained blocked by 25 unrelated concurrent
`workers::cloud` export errors; the requested library-only shape compiled and
passed.
