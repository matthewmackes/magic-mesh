# WL-FUNC-018 App-VM base variable authority — 2026-08-11

- Scope: the App-VM Containerfile must consume exactly the governed build-argument base selected by the build contract.
- Hostile boundary: exactly one `FROM ${APP_VM_BASE}` is required, so a hard-coded substitute base cannot bypass the declared base digest.
- Focused gate: `packaging/app-vm/verify-contract.sh` after isolated farm sync.
- Farm: `172.20.0.50`, isolated slot `appvm-contract-base-r1`, admitted with 8,548,612 KiB free.
- Result: **PASS**, contract checks exited 0 including nested hostile self-tests.
- Remaining boundary: genuine image build/boot with resolved base digest, installed RPM identity, and runtime provenance proof remains.
