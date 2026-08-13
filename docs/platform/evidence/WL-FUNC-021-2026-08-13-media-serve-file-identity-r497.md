# WL-FUNC-021 media serve file identity — 2026-08-13

- Scope: `crates/mesh/mackesd/src/workers/media_server.rs`.
- Reachable gap: the mesh/DLNA scan admitted a regular file into the published
  manifest, but the HTTP `/media/<id>` route retained only its path. Replacing
  that path after the scan could make the authenticated media ID serve bytes
  from an inode that the manifest never admitted.
- Implementation: the serving map now binds each admitted path to its device,
  inode, size, modification time, and change time. The request path opens with
  `O_NOFOLLOW`, compares the opened descriptor with that identity before the
  read, and re-attests it after the read. A replaced, symlinked, or concurrently
  changed file fails closed as unavailable; no replacement bytes are returned.
- Hostile regression: `route_rejects_media_replaced_after_manifest_admission`
  scans a real media file, replaces its path with a symlink to private bytes,
  and proves the reachable HTTP route returns 404 without disclosing them.

## Farm verification

- `.90`, workspace `func021-media-serve-identity-test-r497`:
  `cargo test -p mackesd --features async-services --lib workers::media_server::tests::route_rejects_media_replaced_after_manifest_admission -- --exact --nocapture`
  passed **1/1** with 4,943 filtered and exit 0. The initial wrapper connection
  exited 255 after printing the same green result; the exact warmed-workspace
  rerun supplied the clean process exit recorded here.
- `.130`, workspace `func021-media-serve-identity-module-r497`:
  `cargo test -p mackesd --features async-services --lib workers::media_server::tests -- --nocapture`
  passed **31/31** with 4,913 filtered.
- `.170`, workspace `func021-media-serve-identity-clippy-r497`:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`
  passed.
- `.170`, the same synchronized workspace:
  `rustfmt --edition 2024 crates/mesh/mackesd/src/workers/media_server.rs`
  followed by `rustfmt --edition 2024 --check crates/mesh/mackesd/src/workers/media_server.rs`
  passed; the formatted authorized file was synchronized back before commit.
- `git diff --check` passed.

## Remaining acceptance

This closes the post-scan local file-substitution boundary for the reachable
mesh/DLNA media server. WL-FUNC-021 still requires the deferred post-release
installed-package and physical renderer/provider-loss/cast/handoff/authentication
matrix; those live proofs are not inferred from this farm fixture.
