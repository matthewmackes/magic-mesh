# WL-CRIT-006 publisher-attestation inode stability — 2026-08-11

- Scope: release evidence must authenticate the exact resource-publisher attestation inode admitted for validation.
- Hostile boundary: even a byte-identical atomic pathname replacement during validation is rejected rather than silently authenticating a different inode.
- Focused gate: `install-helpers/release-evidence.sh --self-test` in an isolated farm checkout.
- Farm: `172.20.0.130`, slot 1, admitted with 16,730,512 KiB free.
- Result: **PASS**, self-test exited 0 including the hostile atomic-replacement assertion.
- Remaining boundary: production publisher-HMAC key-store verification, live required-check authenticity, and six-node evidence remain.
