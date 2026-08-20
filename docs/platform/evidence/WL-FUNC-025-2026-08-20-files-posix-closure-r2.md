# WL-FUNC-025 Files POSIX operation closure evidence — 2026-08-20

## Closure assessment

Inspection of source revision `4300596f6c1cd6dec42f6045139189f36d159e3d`
found the WL-FUNC-025 seam already complete; no implementation change was
needed.

- New File uses the shared bounded name dialog and `FileOps::create_file`
  (`O_CREAT|O_EXCL`) with visible refusal on existing names and unwritable
  parents.
- Duplicate stages each selected local row, then submits `OpKind::Copy` to the
  existing operation queue, preserving conflict resolution, progress, cancel,
  and cleanup.
- Compress offers the supported archive formats and submits `OpKind::Compress`;
  Extract Here and Extract To submit `OpKind::Extract` through the same queue.
  Archive traversal, destination-symlink, and cancellation refusal paths are
  implemented in the existing engine.
- Symlink and Hard Link are reachable from both the File menubar's Advanced
  submenu and the row context menu, and execute through `FileOps::symlink` /
  `FileOps::hard_link` with mesh-boundary and cross-device refusal.
- All six operations are present in the background context menu and the
  menubar; queued operations use the existing progress/cancel strip and
  conflict dialog.

## Verification

Current focused farm gate, admitted on `172.20.0.90` slot 2:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2
./install-helpers/xcp-build.sh cargo test -p mde-files-egui --lib \
  model::tests::new_file
3 passed, 0 failed
```

The three tests covered existing-name refusal, read-only-directory refusal for
New File and Duplicate, and local New File/Duplicate/Symlink execution. The
broader backend and archive operation evidence remains in
`WL-FUNC-025-026-2026-08-20-files-posix-prefs-r1.md`, including the
`mde-files` gate and archive round-trip/hostile-path tests.

No live hardware or provider evidence is required by this worklist epic.
WL-FUNC-025 is fold-ready; only the worklist status/archive transition remains
for the owning drain coordinator.
