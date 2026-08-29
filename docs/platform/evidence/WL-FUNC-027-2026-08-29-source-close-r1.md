# WL-FUNC-027 source close — 2026-08-29

Classification: source/cargo close. Operator pin leftover remains
`WL-TEST-003` after a testing Beta.

Source revision: `5f9685408` on `agent/drain-worklist-20260725`.
`production_admitted: false`. No dest invented. No seat mutation.

## Why this closes

S1 is in-tree: `bookmarks.rs` owns a bounded
`<config>/mcnf/files-bookmarks.json` store (cap 48) with pin, rename,
reorder, remove, path validation, and honest refuse/degrade for
hostile, duplicate, corrupt, oversize, and symlink stores. Places
renders the user section above fixed places; mesh peers stay a live
section.

## Farm (current HEAD, not re-run)

Same focused gate as WL-FUNC-026 (do not grind a second copy):

| command | job | ended | result |
|---|---|---|---|
| `cargo test -p mde-files-egui` | `143b09b89c4d` | 2026-08-29T00:52:09Z | 223 passed, 0 failed |

Live leftover: `WL-FUNC-027-2026-08-25-surface-bookmarks-r1.md`.
