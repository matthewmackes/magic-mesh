# WL-FUNC-016 — canonical VDI Files descriptor (r495)

Date: 2026-08-13

## Outcome

The RDP clipboard no longer invents an image-only filename contract or carries
separate shell, transport, and daemon file-metadata shapes. One shared
`VdiClipboardFileDescriptorV1` now bounds the leaf name, optional relative
parent, MIME, byte count, and snapshot count. The live shell admits arbitrary
Files-backed MIME (for example `application/pdf`) through that descriptor,
materializes bytes only after the existing one-use permission/Files authority,
and gives CLIPRDR only the safe leaf name and exact bytes. Guest-to-host staging
uses the same descriptor and therefore cannot drift to a weaker path policy.

The descriptor carries no host path or raw authority. Absolute paths, parent
traversal, controls, malformed MIME, excessive metadata, and size/count claims
outside the rich Files ceiling fail closed. Existing descriptor/hash/length,
lease, replay, lock, range, cancellation, and daemon atomic-commit boundaries
remain in the only production path.

## Exact scope

- `crates/mesh/mackes-mesh-types/src/vdi_clipboard.rs`
- `crates/desktop/mde-vdi-rdp/src/clipboard.rs`
- `crates/desktop/mde-shell-egui/src/vdi/mod.rs`
- `crates/mesh/mackesd/src/ipc/files.rs`

## Farm evidence

- `.170`, `func016-file-descriptor-contract-r495b`: shared descriptor hostile-path/MIME regression passed 1/1.
- `.90`, `func016-file-descriptor-contract-r495c`: non-image `application/pdf` materialization-request regression passed 1/1; strict all-target shared-contract clippy passed.
- `.170`, `func016-file-descriptor-rdp-r495b`: live-connect host Files lock/range/replacement regression passed 1/1; strict production-library clippy passed.
- `.90`, `func016-file-descriptor-files-r495b`: daemon atomic stage/commit/cancel regression passed 1/1 (4,940 filtered).
- `.90`, `func016-file-descriptor-contract-r495c`: strict daemon library clippy with `async-services` passed.
- `.170`, `func016-file-descriptor-shell-clippy-r495`: strict live-VDI shell binary clippy passed.
- `.196`, `func016-file-descriptor-shell-test-r495`: arbitrary `application/pdf` shell projection regression passed 1/1 (1,615 filtered).
- `.170`, `func016-file-descriptor-rdp-r495b`: exact four-file `rustfmt --check` passed after the final source sync.

The first corrected daemon run exposed duplicate serde attributes left where a
local metadata struct was removed; the superseded run failed to compile and is
not acceptance evidence. The first live-connect RDP run exposed a dropped image
bound import and field-to-method conversion; both were corrected before the
green RDP test. A strict all-target RDP clippy attempt reached an unrelated
pre-existing `tests/live_rdp.rs` case-sensitive `.ppm` comparison outside this
slice; the production-library strict clippy gate is the applicable gate.

## Remaining acceptance

Per the operator's release ordering, installed Windows Explorer proof remains
post-release: arbitrary non-image paste, zero-byte files, replacement under
lock, cancellation/expiry/reconnect, bounded seat memory, and confirmation that
no host path is disclosed. This evidence closes the last identified coding
boundary, not that deferred live acceptance matrix.
