# WL-ARCH-010 storage label admission — 2026-08-11

- Scope: physical filesystem command admission.
- Change: filesystem labels are capped at 255 UTF-8 bytes and reject control characters before create, format, relabel, or LUKS-format commands.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=drain-r225-connect-bigboy install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::storage::tests::validate_rejects_oversized_filesystem_label_before_command_admission -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
