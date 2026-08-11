# WL-FUNC-022 — Clock local target admission (r217)

- Scope: locally authored Clock schedules and stopwatch mirrors may target only this node or an approved peer; peer-originated target sets retain governed convergence behavior.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh cargo test -p mackesd local_clock_targets_must_be_self_or_approved_peers --lib -- --exact --nocapture`.
- Result: `.50` passed the focused regression; `git diff --check` passed.
