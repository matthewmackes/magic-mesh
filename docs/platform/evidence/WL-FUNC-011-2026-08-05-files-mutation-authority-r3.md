# WL-FUNC-011 — production Files mutation authority (2026-08-05)

Transfer V2 now commits production Local/Mesh copies through the Files-owned
content-addressed store instead of replacing an existing hash path. The executor
verifies and stages bytes under the source hash, installs the immutable CAS
object, publishes an HMAC-authorized optimistic `CommitFileGeneration`, and
waits for the canonical Files projection before reporting completion.

The collaboration core owns the mutation invariant: the prior generation,
hash, and size must still match; destination name and MIME cannot change; the
replacement hash must be nonzero lowercase SHA-256; and the authored generation
timestamp must strictly advance. Publication, unconfirmed projection,
filesystem, unsupported-authority, and concurrent-generation failures remain
typed and retryable only where corrected-forward recovery is safe.

## Verification

- BigBoy `.130`, slot `wl-func011-files-resolution-test-r3`:
  `cargo test -p mackesd --lib 'workers::transfers::' -- --nocapture`.
- Result: `92 passed; 0 failed; 4382 filtered out`.
- BigBoy `.130`, slot `wl-func011-files-generation-r4`:
  `cargo test -p mde-collab-core file_generation_commit_is_optimistic_and_rejects_a_stale_retry -- --nocapture`.
- Result: `1 passed; 0 failed; 96 filtered out`.
- Final frozen-tree integration, BigBoy `.130`, disposable slot
  `wl-func011-files-resolution-test-r5`:
  `cargo test -p mackesd --lib 'workers::transfers::' -- --nocapture`.
- Result: `92 passed; 0 failed; 4390 filtered out`; the complete current
  `mackesd` library test target compiled. The 4.8 GB slot was removed after
  confirming no process used it.

## Remaining acceptance edge

The current Mesh lane requires canonical source content to be locally
materialized. Real cross-node replication/materialization acknowledgement is
still required. Rsync, SFTP, HTTP, scrape, multipart, recurring mirror, and
Clipboard executors remain typed unsupported.
