# WL-ARCH-010 evidence — validating request capacity exclusion (r218)

- Revision: working tree before commit `r218`
- Scope: live workload admission
- Change: a request already journaled in `Validating` is excluded from its own
  CPU, memory, and storage reservation accounting; other non-terminal
  operations remain counted.
- Farm host: `172.20.0.90`
- Farm slot: `arch010-live-capacity-exclusion-r218-final`
- Gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-live-capacity-exclusion-r218-final install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::workload_compute::tests::validating_request_is_not_counted_against_exact_fit_cpu_capacity -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4746 filtered out`.
- This is focused behavioral coverage; no broad test expansion was added.
