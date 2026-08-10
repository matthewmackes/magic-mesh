# WL-FUNC-016 VDI replay expiry — r182

- Scope: expired VDI clipboard replay lanes are reclaimed before fresh-session admission, while newer replay expiry remains bounded and monotonic.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func016-vdi-replay-expiry-r182 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::clipboard_sync::tests::expired_v2_replay_lanes_release_capacity_before_fresh_admission -- --nocapture`.
- Result: `1 passed; 0 failed; 4713 filtered out` on seat `.90`; expired sessions no longer consume the bounded replay ledger indefinitely. Live VDI proof remains open.
