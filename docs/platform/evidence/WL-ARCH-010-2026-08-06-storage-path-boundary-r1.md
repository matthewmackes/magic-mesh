# WL-ARCH-010 virtual-storage path boundary — 2026-08-06

`virtual_storage.rs` now validates every image path at apply time before an
executor call. The managed root must be an absolute canonical directory;
image paths must be direct managed children with an approved image extension,
and symlinks or outside-root paths are refused. The hostile fixture proves
both a symlink and an outside-root path invalidate without touching the fake
executor.

Verification:

- Farm `.90`, equivalent-patch slot for this slice: focused hostile test
  passed **1/1**.
- BigBoy `.130`, slot `arch010-storage-path-boundary-20260806-r1`, reached
  final compilation but hit `ENOSPC` writing incremental query-cache output;
  no BigBoy pass is claimed.
- The agent’s final `.90` rerun also hit `ENOSPC` after the passing result;
  the host reported approximately 467 MiB free.
- `git diff --check` passed. Source SHA-256:
  `69f357b1bf363c289ed71a7b558e4f82d72fd8720c2b6e8f3ba878e00dd1bd11`.

No destructive storage operation was performed. Live capacity, XFS/SELinux,
libvirt/Quadlet, package, and Dell/seat acceptance remain open.
