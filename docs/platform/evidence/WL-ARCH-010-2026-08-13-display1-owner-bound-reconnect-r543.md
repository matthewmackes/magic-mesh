# WL-ARCH-010 — owner-bound Display1 transport reconnect (r543)

Date: 2026-08-13

## Production result

The node-local Display1 broker now binds an attachment nonce to the kernel
credentials of the first shell process that presents it. That exact process may
reconnect after its prior `SOCK_SEQPACKET` transport is observably dead. A live
relay cannot be displaced, and a process with substituted kernel credentials
cannot replay the nonce.

Replacing a dead transport clears frame/readiness state and advances the input
epoch exactly once. The Workload input loop therefore releases retained guest
edges before it admits input from the recovered transport. This adds no network
transport, fake frame, synthetic audio, or host fallback.

## Hostile regression

`display1_broker::tests::dead_transport_reconnect_requires_same_owner_and_rejects_live_takeover`
exercises the production nonce broker and attachment sink together. It proves
that a candidate cannot replace a live relay, that the same kernel owner can
recover only after transport loss, and that reconnect advances lifecycle
authority once. The adjacent substituted-peer regression proves cross-process
nonce replay remains rejected.

## Farm gates

- BigBoy `.130`, slot 3: exact focused regression discovered and passed 1/1;
  4,985 tests were filtered out.
- `.50`, slot 1: strict all-target `mackesd` Clippy with `async-services`
  passed with `-D warnings`.
- `.170`, slot 2: all-target `mackesd` build with `async-services` passed.
  Its first attempt reached linking but the concurrently saturated host removed
  linker/temp outputs at 2 GiB free; after the owning jobs released capacity,
  the same warmed lane completed successfully with 14+ GiB free.
- `.50`, slot 2: package format check found only pre-existing drift in
  `src/bin/mackesd/spawn.rs`; formatter output for the owned broker hunks was
  applied, while unrelated formatting was preserved.
- `.50`, slot 2: exact `display1_broker.rs` Rustfmt passed after applying the
  owned formatter output.
- Scoped `git diff --check` passed.

## Residual acceptance

The first release still needs its signed App VM image/runtime integration.
Native KMS presentation, physical audio, clipboard, reconnect, cleanup, and
one-node recovery proof remain deferred, non-blocking post-release acceptance.
