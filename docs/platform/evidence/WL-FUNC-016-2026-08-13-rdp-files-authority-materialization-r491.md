# WL-FUNC-016 — RDP guest Files-authority materialization (r491)

Date: 2026-08-13

## Implemented boundary

The live RDP pump now forwards admitted CLIPRDR file-range requests and returns
bounded guest-file chunks to the shell. The shell opens a transaction with the
root-local Files authority, stages each sequential chunk without exposing host
paths, and submits the resulting opaque Files reference through the existing
clipboard permission contract before publishing it.

The Files authority validates file metadata, relative paths, offsets, per-chunk
and aggregate bounds, stages under a transaction-private directory, and
atomically publishes the completed directory with no-replace semantics. Explicit
cancellation, permission refusal, transport failure, timeout, and connection
shutdown remove partial staging; transaction drop and TTL expiry provide the
fail-closed fallback. Concurrent completed destinations are preserved.

## Farm evidence

- `.130`, slot `func016-guest-files-rdp-r491`:
  `cargo test -p mde-vdi-rdp --features live-connect clipboard::tests::guest_file_retrieval_is_sequential_chunked_and_snapshot_bound -- --exact --nocapture`
  passed 1/1; `cargo clippy -p mde-vdi-rdp --features live-connect --lib -- -D warnings`
  passed. The initial all-target clippy reached only the existing out-of-scope
  `tests/live_rdp.rs` case-sensitive-extension lint.
- `.196`, slot `func016-guest-files-shell-r491`:
  `cargo test -p mde-shell-egui --features live-vdi vdi::guest_files_materialization_tests::staged_guest_files_enter_the_permission_contract_without_host_paths -- --exact --nocapture`
  passed 1/1; `cargo clippy -p mde-shell-egui --features live-vdi --bin mde-shell-egui -- -D warnings`
  passed.
- `.90`, slot `func016-guest-files-daemon-r491`:
  `cargo test -p mackesd --features async-services ipc::files::tests::guest_clipboard_files_stage_commit_and_cancel_atomically -- --exact --nocapture`
  passed 1/1, proving nested multi-file atomic commit and partial cancellation
  cleanup.
- `.130`, slot `func016-guest-files-daemon-clippy-r491`:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`
  passed.
- `.170`, slot `func016-guest-files-fmt-r491`:
  `rustfmt --edition 2021 --check crates/desktop/mde-vdi-rdp/src/connect.rs crates/desktop/mde-shell-egui/src/vdi/mod.rs crates/mesh/mackesd/src/ipc/files.rs`
  passed on the final source.

## Remaining epic acceptance

Host-to-guest file serving and post-release local, mesh, and Windows guest proof
remain. Broader user-facing transfer progress/permission presentation may still
need refinement after the first full release; this slice provides the live
permission and progress plumbing but does not claim that post-release proof.
