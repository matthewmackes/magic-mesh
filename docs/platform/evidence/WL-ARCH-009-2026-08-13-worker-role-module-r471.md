# WL-ARCH-009 worker-role module gate — 2026-08-13

- Scope: canonical worker registry, six process-group coverage, role tiers, explicit runtime aliases, responder admission, and fail-closed unknown-worker handling.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-worker-role-module-bigboy-20260813 install-helpers/xcp-build.sh cargo test -p mackesd --locked worker_role::tests -- --nocapture`.
- Result: **PASS**, 31 passed, 0 failed; BigBoy `.130`.
- This module result is independent of the unrelated dirty files currently present in the worktree. The complete mackesd package gate remains separately blocked by 23 failures outside this module.
