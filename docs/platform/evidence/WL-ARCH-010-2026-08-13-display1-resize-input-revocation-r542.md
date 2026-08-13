# WL-ARCH-010 — Display1 resize input revocation (r542)

Date: 2026-08-13

## Production result

The native Display1 DMA-BUF client no longer carries input authority across a
frame-size transition. When a differently sized complete frame arrives while
the guest is focused, the client sends the exact generation-bound `ReleaseAll`,
clears guest focus, and refuses any new focus, key, pointer, or wheel event until
the direct-DRM loop reports successful presentation of that resized frame.

The broker's first-present wire acknowledgement remains one-shot. Subsequent
resize presentation only restores local input authority, so no new protocol or
duplicate readiness side effect was introduced.

The hostile regression exercises a real Unix `SOCK_SEQPACKET` pair and SCM_RIGHTS
frame descriptors: it presents and focuses a 64x32 frame, injects an 80x40 frame,
observes `ReleaseAll`, proves premature refocus is rejected, and proves refocus
is admitted after the new frame is presented.

## Farm evidence

- BigBoy `.130`, slot 1: `cargo test -p mde-shell-egui
  resize_revokes_focused_input_until_the_new_frame_is_presented -- --nocapture`
  compiled the shell test target successfully but selected **0 tests** (1,608
  filtered). This is recorded as an insufficient focused-test result, not a
  passing regression. The operator directed no reruns.
- `.170`, slot 1: strict `cargo clippy -p mde-shell-egui --all-targets
  --all-features --no-deps -- -D warnings` reached the shell and stopped at the
  pre-existing concurrent `communications/mod.rs:608` `while_let_loop` lint. It
  also identified one test-only unnecessary `mut` in this slice, which was
  removed. Per the one-pass/no-rerun directive, Clippy was not rerun.
- `.196`, slot 1: `cargo build -p mde-shell-egui --all-targets --all-features`
  passed. Its synchronized snapshot reported only the since-removed test-only
  `unused_mut` warning.
- `.170`, slot 2: `cargo fmt -p mde-shell-egui -- --check` passed.
- Local scoped `git diff --check` passed after the final correction.

No command was started on `.50`; it was excluded because its `/home` had only
3.4 GiB free and two live jobs.

## Remaining acceptance

- A future permitted gate wave must execute the hostile regression with a
  selector that discovers it; this record does not claim that test passed.
- The first full release must consume the signed App VM image/profile and native
  Display1 broker/runtime path.
- Direct-KMS frame presentation, resize/input behavior, audio, reconnect,
  cleanup, and one-node live acceptance remain deferred and non-blocking until
  after that release.
