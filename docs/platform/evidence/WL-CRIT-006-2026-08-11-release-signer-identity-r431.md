# WL-CRIT-006 release signer identity — 2026-08-11

- Scope: release publication is bound to exactly one governed primary signing fingerprint.
- Hostile boundary: ambiguous/substituted signer identity rolls back all signer-owned outputs before publication.
- Focused gate: `install-helpers/sign-release.sh --self-test` on the farm-synced tree.
- Farm: `172.20.0.196`, slot 1, admitted with 12,247,312 KiB free.
- Result: **PASS**, artifact, signer-identity, and atomic-rollback self-test passed.
- Remaining boundary: exercise the operator keyring/HSM across signing-subkey rotation and verify the governed primary fingerprint.
