# WL-ARCH-009 registry and app-catalog quiescence — 2026-08-09

## Outcome

Unknown worker names now fail the executable role gate instead of inheriting a
Lighthouse rank and passing. `spawn_tiered` resolves the canonical restart
policy before the role gate, preserving its hard failure for an uncensused
programming error while every other caller receives a clean `false` admission.

The Flatpak app catalog worker now waits solely for shutdown when its signed
catalog trust anchor is absent. It no longer opens Bus state or wakes once per
second to repeat an unavailable result, and cancellation remains prompt.

Stale exact worker/group/policy cardinality assertions were removed because
they failed when legitimate registry rows changed without protecting runtime
behavior. The operational guards remain: all six groups must be non-empty,
service names unique, every registry row covered once, every restart policy and
both role tiers represented, unknown names refused, and production start sites
must match the canonical registry bidirectionally.

## Farm verification

- BigBoy (`172.20.0.130`), slot `arch010-s6-duplicate-socket-r2`:
  `cargo test -p mackesd --lib --features async-services worker_role::tests
  --locked -- --nocapture` passed 26/26 after the stale cardinality-only
  assertions were replaced by structural invariants.
- Machine 9 (`172.20.0.50`), slot `arch009-app-catalog-quiescence-r1`:
  `cargo test -p mackesd --lib --features async-services
  workers::app_catalog::tests --locked -- --nocapture` passed 7/7.
- Machine 193 (`172.20.0.90`), slot `arch010-r12-lints`: final app-catalog and
  spawn-source `rustfmt --check`, worklist lint and self-test, workload-authority
  lint, and documentation-supersession lint passed. The touched worker-role
  hunks match rustfmt output; an unrelated pre-existing navigation/clock table
  layout keeps the entire file from passing a whole-file rustfmt check.

## Source hashes

- `5b31c159d1a81afc5e82e0a35e34d92123bef58b4b9135dc6b636a135ca60a68`
  — `crates/mesh/mackesd/src/workers/app_catalog.rs`
- `9ecbb33e7a389acc13bb6aa678800146e63db1c94e9c99b20b66677c44a96147`
  — `crates/mesh/mackesd/src/worker_role.rs`
- `0e89fc73ed3c128443ab7d6fc366eee7ecae24f80a8b7325fd5fe0c1852ce36a`
  — `crates/mesh/mackesd/src/bin/mackesd/spawn.rs`

## Remaining boundary

The registry and one more optional provider are fail-closed, but other optional
providers, complete ownership/output declarations, live process census, Workers
UI cutover, and fleet chaos proof keep WL-ARCH-009 `Remaining`.
