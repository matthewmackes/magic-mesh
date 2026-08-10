# WL-ARCH-010 start-failure attachment revocation — r172

- Scope: `StartAndAttach` revokes its Display1 attachment when the backend start command fails; the VM definition remains eligible for bounded retry without retaining an unserved capability.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-start-failure-revoke-r172 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::start_and_attach_cannot_complete_without_a_real_attachment_lease -- --nocapture`
- Result: `1 passed; 0 failed; 4708 filtered out` on BigBoy; the gate compiled the changed actuator.
