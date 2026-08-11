# WL-ARCH-010 compute expose command bounds — 2026-08-11

- Scope: firewalld, NetworkManager, and interface probes use the shared 15-second subprocess boundary.
- Farm: `.90` `172.20.0.90`, slot `arch010-compute-expose-bound-r234-exact`.
- Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch010-compute-expose-bound-r234-exact install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::compute_expose::tests::bounded_probe_fails_closed_when_child_hangs -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed. The broader module run had one unrelated timing-sensitive Bus-resolution failure (32 passed, 1 failed).
