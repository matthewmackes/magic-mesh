# WL-ARCH-009 Workers This Node authority cutover — r492

Date: 2026-08-13

## Implemented boundary

`Surface::Workers` no longer exposes both the legacy aggregate `This Node`
plane and the governed This Node leaf catalog as peer destinations. The legacy
`WorkersDestination::ThisNode` variant remains available only for route-alias
normalization, while the visible catalog contains exactly the governed leaf
destinations and opens on its Overview leaf by default. This removes one
reachable duplicate node-management authority without changing Health's
separate modal ownership.

Source:

- `crates/desktop/mde-shell-egui/src/workers_catalog.rs`

## Farm verification

- `172.20.0.90`, slot `arch009-workers-cutover-test-r492`:
  `cargo test -p mde-shell-egui workers_catalog::tests::catalog_is_unique_and_deterministically_sorted -- --exact --nocapture`
  passed 1/1 with 1,587 filtered tests. The regression proves the aggregate
  destination is absent, every governed leaf remains present, IDs and labels
  remain unique, and the Overview leaf is the default.
- `172.20.0.196`, slot `arch009-workers-cutover-clippy-r492`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui --no-deps -- -D warnings`
  passed.
- `172.20.0.90`, slot `arch009-workers-cutover-fmt-r492`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/workers_catalog.rs`
  passed after an explicit farm sync.

The first formatting route to `.50` was refused before sync because `/home`
had 7.2 GiB free, below the helper's 8 GiB safety floor. No safety override was
used; the unique gate moved to `.90`.

## Remaining epic acceptance

This slice proves one Workers catalog cutover boundary only. `WL-ARCH-009`
still requires complete Workers/Action Console sole-authority scans, remaining
duplicate route removal, package/process gates, and the deferred post-release
fleet chaos and live-seat convergence matrix.
