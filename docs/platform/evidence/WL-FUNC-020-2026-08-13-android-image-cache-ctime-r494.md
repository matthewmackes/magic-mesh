# WL-FUNC-020 — Android image-cache change-time binding (r494)

Date: 2026-08-13

## Slice

The production Android provider reused a cached image digest when path, length,
mtime, device, and inode matched. A same-inode, same-length image rewrite could
restore the old mtime and retain that stale signed digest, allowing provider
preflight to publish `Ready` for bytes it had not hashed.

`ProductionAndroidHostProbe` now includes Unix filesystem change-time seconds
and nanoseconds in `ImageFingerprint`. Any in-place content or metadata change
therefore invalidates the digest cache even when the image path, inode, length,
and mtime are preserved. The production preflight call sites already consume
this probe, so the behavior is runtime-reachable during Android placement and
provider refresh.

The hostile regression writes a same-length replacement through the original
inode, restores the original access and modification times, and proves the
second admission digest is recomputed from the replacement bytes.

## Farm evidence

The isolated source workspace was based on `022866fd` and contained only this
slice; unrelated shared-worktree Clock, VDI, shell, and media changes were not
synced into these jobs.

- `.130`, slot `func020-provider-ctime-focused-r494`:
  `cargo test -p mackesd --lib production_image_probe_invalidates_cache_after_same_inode_rewrite -- --nocapture`
  passed 1/1 with 4,933 filtered out.
- `.196`, slot `func020-provider-module-r494`:
  `cargo test -p mackesd --lib workers::cloud::android_provider::tests -- --nocapture`
  passed all 6 provider tests with 4,928 filtered out.
- `.90`, slot `func020-provider-ctime-clippy-r494`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.170`, slot `func020-provider-ctime-fmt-r494`:
  file-scoped `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/cloud/android_provider.rs`
  passed. The initial package-wide formatter exposed unrelated committed
  mackesd formatting drift, so it was replaced by the requested file gate.

## Remaining epic acceptance

This closes one provider provenance/recovery gap. WL-FUNC-020 still requires
release artifacts, remote-session attachment, guest packaging, a real
nested-KVM Cuttlefish run, and the deferred post-release live Android/VDI proof.
