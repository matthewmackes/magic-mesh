# WL-ARCH-010 VM overlay failure cleanup — r167

- Revision: `4bc170cb` (`clean up failed VM overlay creation`).
- Scope: VM definition refuses an already-present unmanaged overlay and removes only the newly-created overlay when image creation, XML construction, or definition staging fails; existing disk bytes are never overwritten or deleted.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-overlay-cleanup-r167 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::existing_vm_overlay_is_not_overwritten_or_deleted -- --nocapture`
- Result: `1 passed; 0 failed; 4707 filtered out` on BigBoy.
