# WL-ARCH-010 secure VM XML staging — r170

- Scope: VM domain XML now uses a unique PID/UUID filename, `create_new`, mode `0600`, and sync-before-virsh; partial staging is removed on write failure.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-xml-staging-r170 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::existing_vm_overlay_is_not_overwritten_or_deleted -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on BigBoy; the gate compiled the changed actuator.
