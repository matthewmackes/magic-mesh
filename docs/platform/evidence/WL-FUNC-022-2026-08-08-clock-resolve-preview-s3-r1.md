# WL-FUNC-022 Clock resolve, preview, and local-file audio S3 — 2026-08-08

The existing Clock-audio protocol now includes bounded typed Resolve and
Preview actions. Preview is isolated from Music queue, history, bookmarks,
volume, playback ownership, and an active alarm; it expires after ten seconds.
Raw paths and URLs remain outside the Clock contract.

`mde-musicd` owns a local-file admission registry. It confines files to the
configured root, rejects symlinks, bounds size and codec, verifies metadata and
SHA-256 integrity, and resolves only stable catalog identity. Missing, stale,
modified, malformed, outside-root, or unauthorized files fail closed. The
engine decodes the admitted file internally without returning a `file:` locator.

## Verification

Machine 196 (`172.20.0.196`), slot
`func022-clock-resolve-preview-s3-r1`:

```text
cargo test --locked -p mde-musicd clock_audio -- --nocapture       # 9 passed
cargo test --locked -p mde-musicd clock_local_file -- --nocapture  # 2 passed
cargo test --locked -p mde-musicd \
  engine::tests::admitted_local_clock_file_decodes_without_a_network_locator \
  -- --exact --nocapture                                           # 1 passed
cargo test --locked -p mackes-mesh-types \
  clock::tests::clock_audio_handoff_is_bounded_and_identity_exact \
  -- --exact --nocapture                                           # 1 passed
```

All gates passed with zero failures; scoped diff checking passed.

## Remaining acceptance gap

Caller/UI import and selection, seat-wide audio ducking, and live PipeWire
hardware proof remain. FUNC-022 stays `Remaining`.
