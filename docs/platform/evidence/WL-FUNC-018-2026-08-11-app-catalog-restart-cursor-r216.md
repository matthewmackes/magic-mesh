# WL-FUNC-018 — durable App-catalog restart cursor (r216)

- Scope: the Flatpak App-catalog importer durably checkpoints its cursor only after committed effects and resumes after restart without replaying committed rows.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func018-catalog-cursor-recovery-r216-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::app_catalog::tests::durable_cursor_skips_committed_rows_after_restart -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4742 filtered out; finished in 0.04s`; compiler reported 340 existing warnings and no errors.
