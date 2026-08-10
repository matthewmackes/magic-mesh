# WL-FUNC-016 VDI replay retention — r185

- Scope: retain replay authority until the longest-lived envelope in a source session can no longer materialize; a newer shorter-lived sequence must not reopen an older replay.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func016-vdi-replay-retention-r185 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clipboard_sync::tests::v2_replay_lane_keeps_longest_expiry_across_newer_sequences -- --nocapture`.
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4719 filtered out` on seat `.90`; the replay ledger retained the older expiry and rejected the stale sequence after the newer sequence's expiry.
- Live-proof limit: no live VDI guest or physical-seat proof was performed; this is a daemon replay-authority regression gate only.
