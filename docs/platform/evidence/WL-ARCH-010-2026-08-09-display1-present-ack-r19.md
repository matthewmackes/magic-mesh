# WL-ARCH-010 authenticated Display1 presentation acknowledgement — r19

Date: 2026-08-09
Proof base: `0ff818db3447d2937e7afa3b350dad0c333eb162`

## Outcome

Workload `StartAndAttach` readiness no longer completes when mackesd merely
delivers a QEMU Display1 DMA-BUF to the shell. The lease-bound broker records
delivery separately and waits for one fixed acknowledgement byte from the
authenticated shell peer. The shell emits that byte exactly once, only after a
validated frame has been received and the DRM path has successfully completed
its KMS modeset/page-flip.

The boundary fails closed:

- an idle acknowledgement socket is not readiness;
- EOF is a disconnect, not an acknowledgement;
- an unexpected byte is rejected;
- the shell refuses to acknowledge before receiving a lease- and
  generation-bound frame;
- an acknowledgement send failure tears down external scanout and fails the
  attachment; and
- lease expiry clears the delivered-frame and presented-frame state and removes
  the broker socket.

This closes the false-positive gap where socket delivery could mark a Workload
`Ready` even if PRIME import, modeset, or page-flip subsequently failed.

## Focused farm verification

All commands used isolated proof trees based on the committed proof base plus
only the three files in this slice. Concurrent Surface firmware and DRM
mode-rebuild work remained outside these proofs.

- BigBoy `172.20.0.130`, slot `display1-present-ack-daemon-r1`:
  exact-file rustfmt passed; `cargo test -p mackesd --features async-services
  --lib display1_broker::tests --locked -- --nocapture` passed 9/9.
- Farm node `172.20.0.50`, slot `display1-present-ack-shell-r1`:
  exact-file rustfmt passed; `cargo test -p mde-shell-egui --features drm
  display1_client::tests --locked -- --nocapture` passed 5/5. The regression
  sends a real SCM_RIGHTS descriptor, refuses a pre-frame acknowledgement, and
  observes exactly one post-frame byte.
- Farm node `172.20.0.90`, slot `display1-present-ack-drm-r1`:
  exact-file rustfmt passed; `cargo test -p mde-egui --features drm drm::tests
  --locked -- --nocapture` passed 22/22. The acknowledgement seam succeeds once
  and propagates refusal as a typed DRM error.
- Scoped `git diff --check` passed.

## Source hashes

- `a8f7d7cb4145cd29089574b792a0fe447ead0466cbfda7f05b39577fe4ee5026`
  — `crates/mesh/mackesd/src/display1_broker.rs`
- `ff9392ae9bee55c6bb341ee240c7656833f1879dfb4f09d2ae58aa787ac71d58`
  — `crates/desktop/mde-shell-egui/src/display1_client.rs`
- `773d6f4697dfc4a79b58816fd15ba7f0fe78568dd86adf16c35ca4cb87dbea96`
  — the isolated, formatted `crates/shared/mde-egui/src/drm.rs` proof file
  containing only this slice over the proof base.

## Remaining boundary

This is deterministic protocol and headless DRM-seam evidence. It does not
claim a real QEMU DMA-BUF was imported and displayed on physical KMS hardware,
nor does it complete input/audio/clipboard, package deployment, restart, or the
required Dell/seat-15 lifecycle matrix. WL-ARCH-010 remains `Remaining`.
