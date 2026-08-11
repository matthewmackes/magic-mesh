# WL-UX-011 — unresolved provider state blocks every control verb (r217)

- Scope: an unresolved provider state (`Unknown` without explanatory evidence) fails closed for Enable, Disable, Reload Module, and Rescan Bus without mutating the sysfs control.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux011-device-admission-r217 install-helpers/xcp-build.sh cargo test -p mackesd unavailable_provider_state_cannot_reach_any_control_seam -- --nocapture`.
- Result: BigBoy passed: `1 passed; 0 failed; 0 ignored; 0 measured; 4743 filtered out` for the library test; all other package test targets ran 0 matching tests.
