# WL-FUNC-016 mesh cursor durability — 2026-08-13

## Scope

The authenticated rich-clipboard mesh adapter now syncs its payload-free
cursor file before the atomic rename and syncs the containing directory after
the rename. A crash cannot report a committed cursor while the new checkpoint
is still only in the page cache. The change is limited to
`crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs`.

## Farm gates

- Farm host `172.20.0.130` / slot
  `func016-mesh-cursor-tests-20260813`:
  `cargo test -p mackesd --locked clipboard_sync::mesh --lib -- --nocapture`
  — PASS, 12 passed, 0 failed, 4,912 filtered.
- The passing set includes cross-process SQLite persistence, restart replay
  recovery, longest-expiry retention, CAS identity/hash admission, replay
  refusal, and cursor checkpoint paths.
- A repository-wide `cargo fmt --check` on `.50` was not used as acceptance for
  this slice: it reported pre-existing formatting differences throughout the
  concurrent dirty worktree. No unrelated files were modified or staged.

## Remaining criteria

WL-FUNC-016 still remains `Remaining`. Guest-to-host VDI adapters, live DRM/
guest proof, full MIME round-trip evidence, package/UI permission integration,
and post-release live cleanup proof remain open. This slice proves durable
mesh cursor publication only; it does not claim the epic complete.
