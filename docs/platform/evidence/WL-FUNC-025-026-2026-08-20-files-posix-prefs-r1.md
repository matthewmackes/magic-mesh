# WL-FUNC-025/026 Files POSIX and folder-preference evidence — 2026-08-20

This record covers the Files model slice exercised from the current source
revision `b1847d19` plus the uncommitted LRU-recency correction in
`mde-files-egui/src/model/mod.rs`. It is implementation and farm-test evidence,
not installed-seat or production-release acceptance.

## Implemented behavior

- New File uses `FileOps::create_file` with create-new semantics.
- Duplicate stages each selected local row and submits the existing queued
  `OpKind::Copy`, preserving the standard conflict path and cleanup.
- Compress and Extract use the existing archive queue, including hostile
  traversal/destination checks and cancellation handling.
- Symlink and hardlink creation use the existing `FileOps` wrappers, with
  mesh-boundary and cross-device refusal paths.
- Folder view, sort, and hidden-file preferences hydrate from and persist to
  the bounded JSON store. A folder visit now refreshes its LRU position, so
  eviction reflects recent navigation rather than only recent mutation.

## Focused farm gates

Farm topology was admitted with all five nodes up and `0/10` heavy slots
active. The source was synced by `xcp-build.sh` to each admitted slot.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3
./install-helpers/xcp-build.sh cargo test -p mde-files-egui --lib \
  model::tests::revisiting_a_folder_refreshes_preference_lru_recency
1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3
./install-helpers/xcp-build.sh cargo test -p mde-files-egui --lib \
  model::tests::new_file
3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=1
./install-helpers/xcp-build.sh cargo test -p mde-files-egui --lib \
  model::tests::folder_prefs
1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2
./install-helpers/xcp-build.sh cargo test -p mde-files --lib
157 passed, 0 failed
```

The backend gate emitted three pre-existing warnings in
`crates/services/mde-files/src/editor_open.rs` (`unused_imports`,
`dead_code`); no test failed.

## Boundary and blockers

The UI menubar/context-menu reachability was already present in the current
tree but was outside this worker's permitted write scope. No UI, bookmark-owned
tests, worklist, release, governance, or unrelated crate files were changed.
Live hardware/provider evidence is not required by these two worklist epics and
was not claimed.
