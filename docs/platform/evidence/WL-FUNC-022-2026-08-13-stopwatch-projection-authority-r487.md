# WL-FUNC-022 — distributed stopwatch projection authority (r487)

Date: 2026-08-13

## Acceptance gap

The Clock surface rendered the first daemon-projected stopwatch. A delivered
peer mirror could therefore mask this node's own stopwatch solely because it
arrived first, while the mirror was presented with enabled Start/Pause, Lap,
and Reset controls even though command admission correctly rejected remote
ownership. This contradicted S4's requirement that mirrors remain visibly
read-only and only their origin can control them.

## Implementation

`crates/desktop/mde-shell-egui/src/timers.rs` now:

- selects the stopwatch whose `origin_node_id` equals the projected local
  `node_id` before considering a peer mirror;
- falls back to a delivered mirror when no local stopwatch exists;
- identifies that fallback as `Mirrored from <origin> · read-only on this node`;
- disables Start/Pause, Lap, and Reset at the rendering boundary for mirrors;
- retains the signed command-path ownership check as defense in depth.

The focused regression supplies a peer mirror before a local stopwatch and
proves local authority wins independently of transport order, then removes the
local stopwatch and proves the mirror projection is read-only.

## Farm gates

- BigBoy `172.20.0.130`, slot
  `func022-stopwatch-authority-test-r487`: the final exact command executed
  `timers::tests::local_stopwatch_authority_wins_over_an_earlier_peer_mirror`;
  **1 passed, 0 failed, 1,584 filtered out**. Two earlier command attempts
  matched zero tests and are explicitly not counted as evidence.
- `172.20.0.90`, slot
  `func022-stopwatch-authority-bin-clippy-r487b`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `172.20.0.50`, slot
  `func022-stopwatch-authority-file-fmt-r487c`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/timers.rs`
  passed.

The broader all-target clippy command compiled the changed Clock path but then
reported pre-existing failures in `car_keymap.rs`, `status_bar.rs`, and
`system/mesh.rs`. Package-wide fmt likewise reported concurrent drift only in
`iac/mod.rs` and `workload_api.rs`. Those paths were outside this slice's write
authorization and were not changed; neither failed result is presented as a
green gate.

## Remaining epic acceptance

This closes the local-versus-mirror presentation authority gap. FUNC-022 still
requires post-release multi-process/package/live-seat proof and reviewed
deterministic Clock captures across the full required layout, appearance, input,
and failure-state matrix.
