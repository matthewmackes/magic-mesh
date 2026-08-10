# WL-FUNC-022 stopwatch elapsed deadline — r161

- Revision: `7caa526a`
- Scope: running stopwatches whose live elapsed time exceeds the one-year bound are rejected during command admission and persisted recovery.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func022-stopwatch-deadline-r161 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clock::tests::running_stopwatch_past_elapsed_deadline_is_not_admitted -- --nocapture`
- Result: `1 passed; 0 failed; 4702 filtered out` on BigBoy.

