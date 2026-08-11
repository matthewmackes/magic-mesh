# WL-CRIT-006 finalizer candidate inode — 2026-08-11

- Scope: final publication seals the verifier-approved candidate's exact directory sets, inodes, metadata, and bytes.
- Hostile boundary: post-verification artifact replacement fails even when replacement bytes are identical.
- Focused gate: `python3 install-helpers/finalize-surface-stack.py --self-test` on a clean farm sync.
- Farm: clean sequential rerun on `172.20.0.170`, slot 1, admitted with 11,271,432 KiB free.
- Result: **PASS**, 17 hostile fixtures rejected.
- Remaining boundary: substitute a real candidate artifact between verification and installed publication.
