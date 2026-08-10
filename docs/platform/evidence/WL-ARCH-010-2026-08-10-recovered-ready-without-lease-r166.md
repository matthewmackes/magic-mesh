# WL-ARCH-010 recovered-ready lease authority — r166

- Revision: `bec1b5fe` (`refuse recovered ready workloads without leases`).
- Scope: a recovered terminal `StartAndAttach` record reporting `Ready` without a journaled Display1 lease is revoked before adapter recovery or projection.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch010-missing-lease-r166 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::recovered_ready_without_journaled_lease_is_refused_and_unpublished -- --nocapture`
- Result: `1 passed; 0 failed; 4706 filtered out` on BigBoy.
