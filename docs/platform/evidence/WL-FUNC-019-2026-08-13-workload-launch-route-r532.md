# WL-FUNC-019 workload Launch route — 2026-08-13

Remote Sessions previously admitted generation-bound `Start` and `Resume`
actions but discarded the `launch-g<generation>` action already emitted for a
running Workload. The shell now maps that exact catalog action to the existing
typed `StartAndAttach` Workload operation, reopens authoritative Workload state,
and refuses changed generations, backend substitution, stale actions, and
ambiguous routes before publishing through the resource-action authority.

## Farm gates

- `.196`, slot `func019-launch-r532`: `cargo test -p mde-shell-egui
  running_vm_launch_routes_through_start_and_attach_authority -- --nocapture`
  passed 1/1 (1601 filtered out).
- `.196`, same warmed slot: `cargo clippy -p mde-shell-egui --bin
  mde-shell-egui -- -D warnings` passed.
- BigBoy `.130`, slot `func019-launch-fmt-r532b`: `cargo fmt -p
  mde-shell-egui -- --check` passed after applying rustfmt's exact wrapping.
- `git diff --check` passed.

This is pre-release production wiring. Live resource captures and the reduced
one-node loss/rejoin/action recovery proof remain deferred until after the first
full release and are non-blocking for coding.
