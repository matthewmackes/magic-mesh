# WL-FUNC-018 bounded persistence reads — 2026-08-11

- Scope: App catalog recovery and durable cursor loading.
- Change: retained catalog and cursor reads use bounded `take`-based persistence reads before parsing, refusing data beyond their declared limits.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func018-bounded-recovery-read-r224 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::app_catalog::tests::bounded_persistence_reads_reject_data_beyond_declared_limit -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
