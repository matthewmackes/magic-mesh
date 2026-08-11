# WL-CRIT-006 release-evidence inode binding — 2026-08-11

- Scope: release artifact descriptors bind size and digest to one opened inode.
- Hostile boundary: pathname replacement or same-inode mutation during capture fails without replacing prior evidence.
- Focused gate: `install-helpers/release-evidence.sh --self-test` on the farm-synced tree.
- Farm: `172.20.0.90`, slot 1, admitted with 16,629,044 KiB free.
- Result: **PASS**, deterministic binding and fail-closed validation self-test passed.
- Remaining boundary: substitute a real release artifact during candidate capture and prove published evidence retains the prior valid generation.
