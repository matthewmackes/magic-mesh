# WL-FUNC-021 radio URL admission — 2026-08-10 r191

## Correction

Typed Music radio playback now refuses provider locators containing raw
control/whitespace bytes, missing authorities, unsupported schemes, or URL
userinfo. This keeps provider-supplied credentials and malformed request data
out of the native engine/MPRIS stream path. Valid HTTP(S) radio URLs remain
admitted, including query-bearing stream tokens.

## Farm proof

- Host: `172.20.0.90`
- Slot: `func021-radio-url-admission-r191`
- Command: `cargo test -p mde-musicd --lib bus_responder::tests::typed_radio_play_rejects_credentialed_and_control_bearing_urls -- --exact --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 241 filtered out`
- Scope: `crates/services/mde-musicd/src/bus_responder.rs` only

The focused test covers credential-bearing, whitespace/control-bearing,
malformed-authority, unsupported-scheme, and valid URL cases. No live radio
provider, native audio device, or installed-seat playback proof was available;
those WL-FUNC-021 live limits remain open.
