# WL-FUNC-016 — RDP guest image CAS ingest (r534)

Date: 2026-08-13

## Production result

The live RDP adapter already admitted bounded `CF_DIB`/`CF_DIBV5` bytes and
streamed them to the root-owned Files ingress socket. The daemon previously
returned a `files:v2:vdi-guest:*` token without creating the canonical
collaboration CAS object or a matching Files projection. That token could not
satisfy the existing CAS validator and therefore overstated what Files owned.

`GuestFilesAuthority` now gives every admitted transaction an opaque
`FileRefId` and `SpaceId`. At permission-backed commit, a single-file image is
atomically moved into the user's Files destination, copied into
`collab/content/<sha256-prefix>/<sha256>`, re-read under exact byte-count and
SHA-256 bounds, and published as the matching
`state/collab/file-references/<space>` projection linked by `vdi-guest`. The
shell receives that real `FileRefId`, not a fabricated path-like token.

Existing canonical objects are accepted only after type, size, and full digest
revalidation. CAS or projection failure removes the newly committed Files
destination and returns a refusal; no clipboard event is published. An
unreferenced canonical CAS object may remain after a projection write failure,
but it carries no Files authority and is content-addressed/deduplicable.
Multi-file RDP transfer behavior remains unchanged; this slice closes the exact
single-image admission path.

Owned production file:

- `crates/mesh/mackesd/src/ipc/files.rs`

## Farm gates

- `.90`, slot 1: the initial focused compile was aborted after sustained active
  compilation so the lane would not remain pinned.
- `.90`, slot 2: `cargo clippy -p mackesd --features async-services --lib -- -D warnings` — passed.
- BigBoy `.130`, slot 1: `cargo build -p mackesd --features async-services --lib` — passed.
- BigBoy `.130`, slot 1: the first exact focused test reroute was blocked before mackesd
  by concurrent unowned `mde-collab-core/src/pipeline.rs` errors (unresolved
  `crate::value::CallKind` and an over-parameterized `Result` alias).
- BigBoy `.130`, slot 1, clean worktree pinned to `b00fd95e`:
  `cargo test -p mackesd --features async-services ipc::files::tests::guest_clipboard_image_commit_publishes_exact_cas_and_files_identity -- --exact --nocapture`
  — passed 1/1 (`4971` unrelated library tests filtered).
- `.196`, slot 1: package format check exposed broad pre-existing concurrent
  drift; after applying the sole `ipc/files.rs` delta, the owned file no longer
  appeared in rustfmt output. No unowned formatting was changed.
- Local `git diff --check` — passed before commit.

## Residual acceptance

- Live Windows guest proof remains deferred until after the first full release:
  copy real `CF_DIB` and `CF_DIBV5`, confirm the Files row and canonical object,
  then exercise reconnect, cancellation, and cleanup on the reduced one-node
  topology.
- The existing general multi-file RDP transaction still materializes into
  Files but does not mint one aggregate CAS reference; per-file CAS projection
  is outside this exact image slice and remains a separate executable gap.
