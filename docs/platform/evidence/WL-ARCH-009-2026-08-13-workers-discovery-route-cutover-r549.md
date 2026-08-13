# WL-ARCH-009 — Workers Discovery route cutover (r549)

Date: 2026-08-13

## Result

- `shell/goto/discovery` resolves to the typed
  `WorkersDestination::Discovery` leaf.
- The retired `explorer`, case-variant `Explorer`, and `fleet-explorer` deep
  links fail closed instead of activating the legacy `Surface::Explorer`.
- The external action grammar therefore has one navigation authority for
  Discovery: the unified Workers workspace.

## Verification

- BigBoy `.130`, slot 1: module-qualified hostile regression passed 1/1 with
  1,614 tests filtered out:
  `cargo test -p mde-shell-egui --bin mde-shell-egui toast_bridge::tests::discovery_route_uses_workers_and_retired_explorer_aliases_fail_closed -- --exact --nocapture`.
- `.196`, slot 1: one package formatting check was run. It identified existing
  formatting drift in concurrently owned `front_door.rs`, `main.rs`, and
  `splash.rs`, plus one import-order delta in the owned `toast_bridge.rs`.
  The owned delta was corrected exactly; the command was not rerun under the
  stop cadence.
- Strict Clippy on `.50` and the all-target build on `.170` were stopped when
  the operator reduced the cadence to the current unique gate only. Neither is
  claimed as passing evidence.
- Final scoped `git diff --check` passed.

## Residual ARCH-009

Continue retiring remaining Fleet, Workbench, and This Node external aliases;
finish Network Operations projections, provider/action ownership inventory,
and responsive evidence. Installed one-node acceptance remains deferred until
after the first full release.
