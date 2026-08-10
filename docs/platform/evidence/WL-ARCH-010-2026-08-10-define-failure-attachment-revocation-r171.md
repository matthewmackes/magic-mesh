# WL-ARCH-010 definition-failure attachment revocation — r171

- Scope: `StartAndAttach` now revokes its Display1 attachment when VM definition fails after broker acquisition, preventing a stranded lease during retry or terminal failure.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-attachment-revoke-r171 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::existing_vm_overlay_is_not_overwritten_or_deleted -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on BigBoy; the gate compiled the changed actuator.
