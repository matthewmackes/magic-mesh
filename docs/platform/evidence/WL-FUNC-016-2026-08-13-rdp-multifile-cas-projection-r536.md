# WL-FUNC-016 — RDP multi-file CAS projection (r536)

Date: 2026-08-13

## Production result

The daemon-owned RDP guest Files ingress previously committed a multi-file
transaction into the desktop Downloads tree but minted no usable Files
identities for it. Only the single-file image case entered collaboration CAS and
the canonical `state/collab/file-references/<space>` projection.

`GuestFilesAuthority` now allocates one opaque `FileRefId` and one bounded
SHA-256 accumulator per admitted file descriptor. It streams each exact file
into the existing root-owned staging transaction, verifies every descriptor's
declared size, installs every file under its independently verified content
address, and publishes the complete set of identities in one Files projection.
Names, MIME types, sizes, hashes, and identities remain bound to the original
admitted descriptor order; no host path is exposed as identity.

The visible Downloads destination is still created by one no-replace rename.
CAS/type/size/hash failure removes the destination and staging tree, and the
single projection write occurs only after every file has passed CAS admission.
Thus a failed later file cannot expose a partial Files projection. A canonical
unreferenced CAS object from an earlier file may remain: deleting a shared
content address during rollback could invalidate a concurrent reference, while
an unreferenced hash carries no authority and is safe for bounded deduplication
or later garbage collection.

Owned production file:

- `crates/mesh/mackesd/src/ipc/files.rs`

## Farm gates

- `.170`, `func016-multifile-clippy-r1`:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings` — passed.
- `.170`, `func016-multifile-success-test-r3`:
  `cargo test -p mackesd --features async-services --lib ipc::files::tests::guest_clipboard_files_stage_commit_and_cancel_atomically -- --exact --nocapture` — passed 1/1 (`4975` unrelated tests filtered).
- `.90`, `func016-multifile-failure-test-r2`:
  `cargo test -p mackesd --features async-services --lib ipc::files::tests::guest_clipboard_multifile_cas_failure_has_no_partial_projection -- --exact --nocapture` — passed 1/1 (`4974` unrelated tests filtered).
- `.196`, `func016-multifile-build-r3`:
  `cargo build -p mackesd --features async-services --lib` — passed.
- `.170`, exact-file Rust 1.94 `rustfmt --edition 2021 --check` — passed after
  applying the four owned deltas identified by the earlier package format run.
  Package-wide format remains red only on concurrent unowned files.
- Scoped `git diff --check` — passed before commit.

An initial BigBoy command was stopped and its workspace removed after audit
showed its short filter plus `--exact` would execute zero tests while building
all targets. It is not counted as evidence.

## Residual acceptance

No known executable multi-file Files/CAS identity gap remains in this ingress
transaction. After the first full release, run the deferred non-blocking live
Windows multi-file copy, reconnect, cancellation, destination cleanup, and
one-node recovery proof. General rich-clipboard post-release acceptance remains
separate from this completed coding slice.
