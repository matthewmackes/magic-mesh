# WL-FUNC-016 — bounded RDP guest file retrieval (r490)

Date: 2026-08-13

## Implemented boundary

`mde-vdi-rdp` now turns an admitted `FileGroupDescriptorW` snapshot into
sequential CLIPRDR `RANGE` requests. Only one range is in flight, every request
is capped at 256 KiB and carries the admitted file index and `clipDataId`, and
short valid responses advance from their exact returned length. The bridge
publishes bounded chunks with file index, offset, and completion state for the
Files authority to persist instead of allocating from the guest-declared full
file size.

Unsolicited, empty, oversized, error, or wrong-stream responses fail closed and
cancel the transfer. Clipboard replacement and expired outgoing locks revoke
the admitted snapshot, pending request, and unpublished chunk together.

## Farm evidence

- BigBoy `.130`, slot `func016-chunk-test-r490`:
  `cargo test -p mde-vdi-rdp --features live-connect guest_file_retrieval_is_sequential_chunked_and_snapshot_bound --locked -- --nocapture`
  passed 1/1 on the final source. The fixture proves a 300,000-byte file is
  reassembled as a short 100,000-byte first response plus an exact 200,000-byte
  tail and that a wrong stream ID is rejected.
- `.90`, slot `func016-chunk-clippy-r490`:
  `cargo clippy -p mde-vdi-rdp --features live-connect --lib --locked -- -D warnings`
  passed. An earlier all-target run reached an unrelated existing
  `tests/live_rdp.rs` case-sensitive-extension lint outside this slice.
- `.196`, slot `func016-chunk-finalfmt2-r490`:
  `rustfmt --edition 2024 --check crates/desktop/mde-vdi-rdp/src/clipboard.rs`
  passed on the final source.

## Remaining epic acceptance

Files-authority destination materialization and cleanup, host-to-guest file
serving, UI permission/progress integration, and post-release local/mesh/Windows
guest proof remain. This checkpoint does not claim those live paths.
