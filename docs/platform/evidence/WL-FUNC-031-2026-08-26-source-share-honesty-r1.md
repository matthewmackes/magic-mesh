# WL-FUNC-031 source share/join/follow/close honesty — r1

Date: 2026-08-26  
Observed: `2026-08-26T12:00:00Z`–`2026-08-26T12:10:00Z`  
Classification: source-unit / farm gate; **not** two-seat live co-edit,
**not** visible-cursor production proof, **not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725`  
Farm HEAD at run: dirty tree on `6bafb6080` parent lineage (`b6fd8aeab` plus
in-flight sibling lanes). This unit did not invent a dest and did not
occupy Seat 15 / Dell / Surface Construct.  
Control host: `rocky9-kvm2`  
`production_admitted: false`

## Source unit

Documents share/join/follow/close honesty and external-write three-way
merge, in:

- `crates/desktop/mde-collab-egui/src/documents.rs`
- `crates/desktop/mde-collab-egui/src/fixture.rs`
- `crates/desktop/mde-editor-egui/src/panel/mod.rs`
- `crates/desktop/mde-editor-egui/src/panel/tests.rs`

Phase-3c markers: **gone** from `documents.rs` (grep empty). Remaining
"Phase 3c" prose in `lib.rs` / `data.rs` / `tests.rs` is outside this
write scope.

What landed:

- Share-session pump publishes the editor caret/viewport so follow-mode
  has a real cursor on the wire, not an empty presence.
- `EditorSurface::replace_text` keeps/clamps the caret instead of jumping
  to end (no silent caret clobber on CRDT apply or external merge).
- Fixture helper `FixtureData::document_share` for two-seat session rows.
- Tests: sequential co-edit without caret jump, concurrent suffix inserts
  keep both lines, unknown-peer follow refuses honestly, overlapping
  external write still surfaces `ExternalWriteConflict`.

## Farm

- Host: `172.20.0.90` (KVM-XCP1), slot `2`, remote `magic-mesh-farm-2`
- Admission: 46 386 996 KiB free (required 8 388 608 KiB)
- Command: `cargo test -p mde-collab-egui`
- Result: **186 passed, 0 failed, 0 ignored** (lib tests 1.71s)
- A peer result `5ee4d77747ec` had already passed the same command at
  `b6fd8aeab-dirty` before this edit; that receipt was not reused
  because this unit changed source. Did not duplicate an in-flight
  admission of the new tree.

## Leftover

Still **`@leftover:{live-seat}` two-seat co-edit**. Closing it needs a
current-SHA collaboration identity dest so `collab` can spawn (FUNC-023;
identity receipt `source_revision` was stale `7e3474eeb` vs installed
dest-cut), plus a second current-revision seat. Seat 15 / Dell / Surface
Construct were not used. Do not invent a dest. Do not flip
`production_admitted`.
