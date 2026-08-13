# WL-FUNC-019 — Windows RDP reconnect generation reset (r541)

## Production result

`mde-vdi-rdp` now opens each successfully authenticated RDP transport as a new
one-use state generation. Before the replacement transport receives focus, the
session discards queued clicks, keys, synthesized held modifiers, and cached
pointer position from the retired connection. It also replaces the old Windows
framebuffer with an opaque-black full-damage frame until the new transport
paints.

This closes a reconnect race where input queued immediately before disconnect
could otherwise act on a different Windows screen, and where pixels from the
retired connection could remain visible without evidence from the replacement
transport. Failed handshakes do not reset the retained session because the
generation boundary runs only after the new handshake succeeds.

The hostile regression paints an old-generation pixel, queues Ctrl+A and a
pointer move, enters a replacement generation, and proves that no input,
modifier, pointer, or pixel authority survives. It then proves the first new key
is cleanly admitted without the retired Ctrl state.

## Farm evidence

- `.170`, slot 1: exact focused regression passed 1/1:
  `cargo test -p mde-vdi-rdp --features live-connect session::tests::replacement_connection_cannot_replay_retired_input_or_pixels -- --exact --nocapture`.
- BigBoy `.130`, slot 1: strict all-target/all-feature Clippy passed:
  `cargo clippy -p mde-vdi-rdp --all-targets --all-features -- -D warnings`.
- BigBoy `.130`, slot 1: production all-target/all-feature build passed:
  `cargo build -p mde-vdi-rdp --all-targets --all-features`.
- `.170`, slot 2: package `cargo fmt -p mde-vdi-rdp -- --check` was run once
  and remains red on pre-existing Rust 1.94 formatting drift in `clipboard.rs`,
  `lib.rs`, and regions of `session.rs` outside this slice. No unrelated
  formatting rewrite was taken.
- Scoped `git diff --check` passed for both production files and this evidence.

## Remaining FUNC-019 acceptance

Pre-release work still includes the final audit of universal-resource action
routes and any concrete Windows RDP clipboard/lifecycle/readiness gaps outside
this reconnect boundary. The first full release must carry the signed runtime
and publisher credentials. Authenticated Windows login/render, reconnect,
clipboard/input, recovery, and one-node live acceptance remain deferred and
non-blocking until after that release.
