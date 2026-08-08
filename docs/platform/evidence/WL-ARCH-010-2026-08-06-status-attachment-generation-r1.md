# WL-ARCH-010 Workload attachment generation validation — 2026-08-06

Workload status validation now rejects an attachment lease whose generation
differs from the authoritative status generation. A stale lease cannot be
projected as the current attachment after a newer operation is accepted.

Verification:

- BigBoy `.130` passed the focused
  `status_rejects_a_stale_attachment_generation` test **1/1**.
- Source SHA-256:
  `fc1800a595477766c2f7914f4a7d2c3b6587bf0b1ac0276d8c7a6d82a0f85cfa`.
- `git diff --check` passed; Dell runtime was not modified.

This is contract-level evidence only; live Display1/KMS generation handoff and
seat recovery remain open.
