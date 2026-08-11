# WL-FUNC-016 RDP duplicate response — r194

- Scope: an unsolicited CLIPRDR format-data response is treated as a replay and cannot erase a previously admitted guest clipboard value when the protocol supplies no response nonce.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func016-rdp-duplicate-response-r194 install-helpers/xcp-build.sh cargo test -p mde-vdi-rdp --features live-connect --lib clipboard::tests::duplicate_remote_response_cannot_erase_admitted_clipboard -- --nocapture`.
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out` on seat `.50`; the admitted value survived an unsolicited duplicate response.
- Live-proof limit: no live Windows guest or physical-seat clipboard proof was performed; this is a focused CLIPRDR replay-authority regression gate.
