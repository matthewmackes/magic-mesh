# WL-FUNC-018 Front Door peer-node admission — r491

Date: 2026-08-13

## Result

Front Door peer-app discovery now validates the focused and replying peer
identity before publishing `action/apps/peer-list`, creating request/cache
authority, or admitting a signed Flatpak catalog. Empty, path-like,
control-bearing, punctuation-bearing, dot-segment, and greater-than-128-byte
identities fail closed. This closes an authority gap where launch admission
eventually rejected an unsafe node but discovery had already emitted a Bus
request and keyed retry/cache state with it.

The changed path is production-reachable from the shell's focused-peer drive;
no host app, command, or alternate launcher path was added.

## Farm gates

- BigBoy `.130`, slot `func018-peer-node-admission-test-r491`:
  `cargo test -p mde-shell-egui unsafe_peer_identity_never_reaches_discovery_or_reply_cache_authority -- --nocapture`
  passed 1/1 (1,586 filtered).
- BigBoy `.130`, slot `func018-peer-node-admission-bin-clippy-r491b`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `.196`, slot `func018-peer-node-admission-fmt-r491b`:
  `cargo fmt -p mde-shell-egui -- --check` passed.

An initial stricter all-target clippy reached the shell but stopped on existing,
out-of-scope test-only findings in `car_keymap.rs`, `status_bar.rs`, and
`system/mesh.rs`. None referenced the changed file. The production binary gate
above remained strict (`-D warnings`) and covers the reachable module.

## Remaining epic acceptance

This slice proves only the Front Door discovery authority boundary. WL-FUNC-018
still requires a current governed App VM image/profile, complete open/resume/
stop and crash cleanup, polished permission/progress/failure UX, and deferred
post-release package, SELinux, VDI, persistence, reconnect, and live-seat proof.
