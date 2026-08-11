# WL-FUNC-021 media configuration bound — 2026-08-11

- Scope: shared-folder configuration loading.
- Change: shared-folders JSON must be a regular file no larger than 64 KiB before parsing; oversized or symlinked config falls back to standard directories.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=codex-media-config-20260811-1 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::media_server::tests::oversized_shared_folder_config_is_rejected_before_json_parse -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
