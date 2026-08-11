# WL-CRIT-006 CI source identity — 2026-08-11

- Scope: an authoritative CI result must describe the immutable committed revision it claims to verify.
- Hostile boundary: dirty, untracked, unresolved, or mid-run-mutated source cannot report working-tree bytes as a green result for `HEAD`.
- Focused gate: `install-helpers/ci-gate.sh --self-test` after an isolated farm sync.
- Farm: `172.20.0.90`, isolated slot `ci-crit006-source`, admitted with 14,335,396 KiB free.
- Result: **PASS**, self-test exited 0 including the hostile uncommitted-substitution assertion.
- Remaining boundary: live GitHub required-check identity and branch-protection enforcement proof remain.
