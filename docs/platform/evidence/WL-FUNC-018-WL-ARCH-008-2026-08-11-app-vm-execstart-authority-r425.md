# WL-FUNC-018 / WL-ARCH-008 App-VM ExecStart authority — 2026-08-11

- Scope: the App-VM guest runtime unit has one active canonical executable authority.
- Hostile boundary: commented canonical decoys cannot mask a substituted active `ExecStart`.
- Focused gate: `packaging/app-vm/verify-contract.sh` on the farm-synced tree.
- Farm: `172.20.0.196`, slot 1, admitted with 14,283,216 KiB free.
- Result: **PASS**, exact contract self-test passed.
- Remaining boundary: boot the image and bind effective unit plus `/proc/<pid>/exe` to the governed guest runtime across cloud-init/restart.
