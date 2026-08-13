# WL-ARCH-009 worker alias ownership — 2026-08-13

- Scope: runtime aliases must resolve only to their registered worker and preserve the canonical process-group boundary.
- Implementation: `belongs_to_group` resolves the explicit runtime alias table; the regression assertion now checks the alias's canonical Control admission and rejects an unrelated Observation group.
- Focused farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-worker-ownership-final-20260813 install-helpers/xcp-build.sh cargo test -p mackesd --locked admitted_runtime_aliases_preserve_process_group_ownership -- --nocapture`.
- Result: **PASS**, 1 passed, 0 failed; farm `.90`.
- Clippy gate: `cargo clippy -p mackesd --locked --lib` on farm `.130`, slot `arch009-mackesd-clippy-fixed-20260813`, exited 0 with warnings only.
- Full coding gate status: the prior `cargo test -p mackesd --locked --lib` run remains **BLOCKED**, with 4,902 passed and 23 failures across unrelated worker/cloud/scheduler/transfer/vehicle tests. This is a coding-gate blocker, not deferred live proof.
