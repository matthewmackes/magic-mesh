# WL-FUNC-021 / WL-ARCH-009 bookmark probe timeout — 2026-08-11

- Scope: bookmark HTTP link probes use the shared bounded subprocess executor.
- Farm: `.50` `172.20.0.50`, slot `func011-bookmark-probe-timeout-r235`.
- Command: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func011-bookmark-probe-timeout-r235 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::bookmarks::tests::link_probe_command_times_out_a_hung_child -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed.
