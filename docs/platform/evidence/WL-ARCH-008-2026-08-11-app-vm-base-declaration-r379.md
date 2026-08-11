# WL-ARCH-008 App-VM base declaration authority — 2026-08-11

- Scope: the shipped App-VM build contract accepts exactly one governed `APP_VM_BASE` declaration.
- Hostile boundary: a later conflicting base declaration cannot override the expected Fedora bootc base while the verifier still passes.
- Focused gate: sync through `MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh sync`, then run `packaging/app-vm/verify-contract.sh` in that farm workspace.
- Farm: `172.20.0.196`, slot 1, admitted with 12,001,012 KiB free.
- Result: **PASS**, one verifier invocation, zero failures; conflicting duplicate-base, provenance, RPM identity/supply, and contract assertions passed.
- Remaining boundary: built/booted immutable-root and installed package/service proof remain.
