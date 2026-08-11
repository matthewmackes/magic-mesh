# WL-FUNC-016 — future VDI clipboard envelope admission (r216)

- Scope: future-dated VDI clipboard envelopes are rejected before replay admission.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func016-vdi-future-envelope-r216 install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib vdi_clipboard::tests::vdi_transport_rejects_future_dated_envelopes_before_replay_admission -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 518 filtered out`.
