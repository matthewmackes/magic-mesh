# WL-ARCH-010 recovered attachment lease deadline — r158

- Revision: `6d986674`
- Scope: recovered attachment leases are rejected when they outlive the originating operation deadline.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-workload-deadline-r158 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::recovered_attachment_lease_cannot_outlive_its_operation_deadline -- --nocapture`
- Result: `1 passed; 0 failed; 4698 filtered out` on BigBoy.

