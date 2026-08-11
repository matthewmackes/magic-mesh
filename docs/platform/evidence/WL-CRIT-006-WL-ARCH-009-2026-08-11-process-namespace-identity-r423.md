# WL-CRIT-006 / WL-ARCH-009 process namespace identity — 2026-08-11

- Scope: production mackesd units execute the packaged binary in the host filesystem namespace.
- Hostile boundary: root/image/bind/tmpfs/mount/extension directives cannot substitute `/usr/bin/mackesd` while preserving canonical `ExecStart` text.
- Focused gate: `python3 install-helpers/verify-mackesd-process-boundary.py --self-test` after farm sync.
- Farm: `172.20.0.196`, slot 1; admission reported 14,284,112 KiB free.
- Result: **PASS**, focused self-test passed.
- Remaining boundary: inspect installed merged units/transient properties and bind live `/proc/<pid>/exe` inode/digest to the installed RPM across upgrade.
