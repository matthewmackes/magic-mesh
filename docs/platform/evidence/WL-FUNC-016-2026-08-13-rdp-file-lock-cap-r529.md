# WL-FUNC-016 — bounded RDP file-lock ownership (r529)

Date: 2026-08-13

## Production gap

The live IronRDP CLIPRDR backend retained one `LocalFileOffer` for every
peer-selected `LockDataId`. The map had no capacity bound and did not sweep
expired snapshots when a new lock arrived. A connected guest could therefore
retain unbounded references to permission-approved delayed-rendering bytes.

## Change

`crates/desktop/mde-vdi-rdp/src/clipboard.rs` now caps concurrent locked local
file snapshots at 16. Lock admission first removes snapshots older than the
existing serving TTL. At capacity, a new peer-selected ID retains no bytes and
subsequent SIZE/RANGE requests fail closed. Existing admitted transfers are not
revoked by overflow, and an explicit unlock immediately releases capacity.

This is transport-layer ownership cleanup only. It does not claim live guest or
post-release hardware proof.

## Farm evidence

- `.130`, slot `func016-rdp-lock-test-r529`:
  `cargo test -p mde-vdi-rdp --features live-connect clipboard::tests::host_file_lock_ownership_is_bounded_and_released -- --exact --nocapture`
  passed 1/1.
- `.170`, slot `func016-rdp-lock-clippy-r529`:
  `cargo clippy -p mde-vdi-rdp --features live-connect --lib -- -D warnings`
  passed.
- `.50`, slot `func016-rdp-lock-fmt-r529`:
  `rustfmt --edition 2024 --check crates/desktop/mde-vdi-rdp/src/clipboard.rs`
  passed.
- Local `git diff --check` passed.

An additional `cargo clippy -p mde-vdi-rdp --features live-connect
--all-targets -- -D warnings` attempt compiled the changed library but stopped
on the pre-existing `clippy::case_sensitive_file_extension_comparisons` warning
in unowned `crates/desktop/mde-vdi-rdp/tests/live_rdp.rs:125`. That unrelated
test file was intentionally not changed under this slice's clipboard-only
ownership.

## Remaining epic acceptance

- Connect and prove rich clipboard behavior on the post-release one-node live
  DRM/mesh/VDI stack.
- Exercise permission, revocation, reconnect, and cleanup against real guest
  adapters after the first full release.
- Retain the first-release artifact and runtime evidence required by the active
  worklist; this slice does not close WL-FUNC-016.
