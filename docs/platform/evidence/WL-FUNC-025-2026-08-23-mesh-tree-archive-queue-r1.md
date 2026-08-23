# WL-FUNC-025 mesh-tree and archive-queue evidence — 2026-08-23

This record covers the leftover S1/S2/S3 farm-testable slice for unit
`143b09b89c4d` (`cargo test -p mde-files-egui`). It is implementation and
farm-test evidence, not installed-seat or production-release acceptance.
Live Files use on a current-revision seat remains a `WL-REL-002` dependency.

## Slice

S1–S3 command wiring was already in-tree. The leftover farm-testable slice
was execution on a **fixture** lock-11 mesh-mount path and a complete zip
**and** tar.gz queue round-trip. The path is a local tempdir shaped like
`…/run/user/1000/mde-mesh/oak/docs` and served by `LocalFsBackend`. It is
not a live overlay peer or installed-seat Files tree.

- New File, Duplicate, hard link, and symlink now run through
  `LocalFsBackend` + `LiveFileOps` on that fixture path. After each
  success, reload lists the created row and `symlink_metadata` reports the
  created link.
- `zip_and_tar_gz_round_trip_through_the_queue` now extracts both archives
  through `OpKind::Extract` into separate destinations.

Write scope: `crates/desktop/mde-files-egui/src/model/tests.rs` only.

## Focused farm gate

Admitted on `172.20.0.90` slot 1 (84714784 KiB free; required 8388608 KiB).
No ENOSPC.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1
./install-helpers/xcp-build.sh cargo test -p mde-files-egui
208 passed, 0 failed
```

Including:

- `model::tests::new_file_duplicate_and_links_execute_on_a_mesh_mounted_tree`
- `model::tests::zip_and_tar_gz_round_trip_through_the_queue`
