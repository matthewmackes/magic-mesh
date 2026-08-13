# WL-FUNC-016 RDP guest file-list admission — r489

Date: 2026-08-13

## Implemented boundary

The live `mde-vdi-rdp` CLIPRDR backend now negotiates the registered
`FileGroupDescriptorW` format instead of leaving every file callback as a
no-op. Guest metadata is admitted only when it belongs to the pending exact
registered format and carries a bounded, non-empty list of safe relative names
with known sizes. Registered-format equivocation, unsolicited/replaced
responses, parent/absolute paths, missing sizes, aggregate content above the
canonical 4-GiB rich-envelope ceiling, and lists above 4,096 entries fail
closed with `ClipboardBridgeError::InvalidFileList`.

An admitted list retains IronRDP's `clipDataId`, binding later chunk retrieval
to that clipboard snapshot. The typed snapshot is removed when IronRDP reports
that its outgoing lock was cleared, so expired clipboard authority cannot be
reused. Raw guest paths and file bytes are not materialized by this transport
boundary.

## Farm evidence

- Host `.130` (XEN-BIGBOY), slot
  `func016-rdp-file-admission-test-r489`:
  `cargo test -p mde-vdi-rdp --features live-connect --lib clipboard::tests::guest_file_list_is_format_bound_bounded_and_lock_scoped -- --exact --nocapture`
  passed 1/1 with 109 filtered tests. The regression exercises valid nested
  metadata, traversal refusal, aggregate-size refusal, lock cleanup, and
  registered-format equivocation.
- Host `.130`, slot `func016-rdp-file-admission-clippy-r489`:
  `cargo clippy -p mde-vdi-rdp --features live-connect --lib -- -D warnings`
  passed.
- Host `.130`, slot `func016-rdp-file-admission-fmt-r489`: direct farm command
  `rustfmt --edition 2021 --check crates/desktop/mde-vdi-rdp/src/clipboard.rs`
  passed. The direct file check was used because package fmt also reports
  unrelated pre-existing drift outside this slice's authorized file.

The same focused test and strict clippy were rerun from the retained warm r489
slots after the final file formatting; both completed successfully in under one
second. Initial `.170` routing was refused before source sync because `/home`
had 6.4 GiB free, below the farm helper's 8-GiB safety floor. Obsolete r488
workspaces were removed before the final rerun.

## Remaining epic acceptance

This slice closes bounded RDP file-list negotiation/admission and lock-scoped
metadata cleanup. Chunked file-content requests, Files-authority
materialization, host-to-guest file publication, and post-release live Windows
round-trip proof remain. The broader epic also still requires its deferred
local/mesh/VDI release evidence bundle.
