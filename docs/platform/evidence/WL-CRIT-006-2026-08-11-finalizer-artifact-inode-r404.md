# WL-CRIT-006 finalizer artifact inode stability — 2026-08-11

- Scope: Surface-stack finalization must hash and publish the exact artifact inode admitted for the candidate.
- Hostile boundary: hashing uses one `O_NOFOLLOW` descriptor and rejects even a byte-identical atomic pathname replacement before publication.
- Focused gate: `install-helpers/finalize-surface-stack.py --self-test` after isolated farm sync.
- Farm: `172.20.0.170`, slot 1, admitted with 12,274,004 KiB free.
- Result: **PASS**, self-test exited 0 with 16 hostile fixtures rejected.
- Remaining boundary: genuine signed Surface RPM finalization and downstream release-evidence identity proof remain.
