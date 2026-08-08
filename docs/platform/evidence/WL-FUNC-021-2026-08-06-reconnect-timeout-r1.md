# WL-FUNC-021 bounded Music reconnect request timeout — 2026-08-06

Resumed Subsonic requests now use a bounded connect timeout and finite request
deadline. A provider that sends headers and then stalls cannot pin the daemon's
decode/recovery thread indefinitely; ordinary live-stream requests retain their
separate streaming behavior.

Verification:

- BigBoy `.130`, slot `musicd-reconnect-timeout-20260806-r1`:
  `cargo test --locked -p mde-musicd --lib reconnect_request -- --nocapture`
  passed **2/2** (177 filtered).
- `git diff --check` passed.
- Source SHA-256: `engine.rs`
  `ba13b0a5a3ea63d3ea3289099e755f79b63ac3ae15fb99c403e4463698bfc340`,
  `reconnect.rs`
  `14a097726073a7b3fe864a799bd8fbf38c066e62ba2b125ccceca7ae1abd79d7`.

Live provider outage/reconnect continuity, rendered acceptance, package, and
seat proof remain open. Dell runtime was not modified.
