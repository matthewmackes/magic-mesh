# WL-FUNC-016 RDP locked-file revocation — r504

Date: 2026-08-13

## Result

The RDP clipboard backend now destroys every lock-bound host-file snapshot,
queued file response, and pending local format request when a replacement is
rejected and the local offer is revoked. A peer can no longer continue serving
bytes from a previously approved file lock after invalid replacement metadata
has cancelled host-to-guest authority.

Ordinary valid replacement retains CLIPRDR's delayed-rendering snapshot
semantics until unlock. The new revocation path is deliberately stricter: it
increments the local generation and atomically clears the current offer,
advertised generation, pending request, locked snapshots, and queued responses.

Changed production scope:

- `crates/desktop/mde-vdi-rdp/src/clipboard.rs`

## Farm gates

- BigBoy `172.20.0.130`, slot `func016-revoke-test`:
  `cargo test -p mde-vdi-rdp --features live-connect host_file_serving_is_permission_bounded_range_bound_and_cancelled -- --nocapture`
  passed 1/1, with 111 unrelated unit tests filtered.
- `172.20.0.50`, slot `func016-revoke-clippy`:
  `cargo clippy -p mde-vdi-rdp --features live-connect --lib -- -D warnings`
  passed.
- `172.20.0.170`, slot `func016-revoke-fmt`:
  file-scoped `rustfmt --edition 2021 --check
  crates/desktop/mde-vdi-rdp/src/clipboard.rs` passed.
- `git diff --check` passed.

The broader `--all-targets` Clippy attempt reached the crate but was blocked by
an unrelated pre-existing warning in `tests/live_rdp.rs:125` about a
case-sensitive `.ppm` extension comparison. That out-of-scope file was not
changed; strict production-library Clippy covers this production patch.

## Remaining acceptance

FUNC-016 still requires first-release package integration and the deferred,
non-blocking post-release installed proof of text, HTML, image, and file
round-trips across direct DRM, authenticated mesh, and a live RDP guest,
including disconnect/reconnect, permission cancellation, bounded memory, and
cleanup. This focused gate does not claim that live proof.
