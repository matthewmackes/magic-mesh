# WL-FUNC-018 / WL-ARCH-008 App-VM base image ID — 2026-08-11

- Scope: App-VM image construction binds its base and output to immutable image IDs.
- Hostile boundary: a mutable base/output tag retarget during the build cannot substitute verification or disk conversion inputs.
- Focused gate: `packaging/app-vm/build-image.sh --self-test` on the farm-synced tree.
- Farm: `172.20.0.196`, slot 1, admitted with 14,283,180 KiB free.
- Result: **PASS**, exact self-test passed.
- Remaining boundary: retarget a real registry base tag during build and prove the resulting boot disk uses the originally captured base ID.
