# WL-ARCH-010 virtual-storage output bound — 2026-08-11

- Scope: live qemu-img command execution.
- Change: qemu-img now uses the shared timeout-bounded, concurrently drained subprocess capture, retaining at most 64 KiB per output stream.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=drain-r225-connect-bigboy install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::virtual_storage::tests::live_command_output_is_bounded_per_stream -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
