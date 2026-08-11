# WL-FUNC-022 evidence — single Clock sample (r221)

- Scope: Clock tick processing.
- Change: one captured wall-clock value is reused across validation, command
  processing, convergence, and audio publication within a tick.
- Farm host: `172.20.0.90`.
- Farm slot: `func022-single-clock-sample-r221`.
- Gate:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func022-single-clock-sample-r221 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::clock_tick_reuses_one_wall_clock_sample_after_loading -- --exact --nocapture`
- Result: `1 passed; 0 failed; 4752 filtered out`.
