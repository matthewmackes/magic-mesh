# WL-ARCH-010 Quadlet attachment cleanup — r173

- Scope: when Quadlet `StartAndAttach` attachment setup fails after unit materialization, the managed unit is removed; original setup failures and cleanup failures remain explicit.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-quadlet-attachment-cleanup-r173 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::start_and_attach_cannot_complete_without_a_real_attachment_lease -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on BigBoy; the gate compiled the changed actuator.
