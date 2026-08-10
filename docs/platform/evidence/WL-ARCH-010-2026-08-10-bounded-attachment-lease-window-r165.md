# WL-ARCH-010 bounded attachment lease window — r165

- Revision: `df62e675` (`bound native attachment lease lifetime`).
- Scope: a native attachment lease cannot outlive the 15-minute Workload operation deadline; expired and overlong capabilities fail contract validation before publication or handoff.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch010-lease-window-r165 install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib workloads::tests::attachment_lease_rejects_an_unbounded_expiry_window -- --nocapture`
- Result: `1 passed; 0 failed; 514 filtered out` on seat 50.
