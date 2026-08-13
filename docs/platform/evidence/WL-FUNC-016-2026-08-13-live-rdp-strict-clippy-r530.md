# WL-FUNC-016 live RDP strict-Clippy unblock — r530

Date: 2026-08-13

The live RDP qualification target failed strict all-target Clippy because its
optional framebuffer-capture helper compared the `.ppm` extension
case-sensitively. The helper is live code used by qualification and menu-reaction
captures, so it was retained. Extension admission is now case-insensitive and
the derived variant capture still receives a canonical lowercase `.ppm`
extension. No lint allowance or duplicate test was added.

Farm evidence:

- `172.20.0.170`, slot `func016-live-rdp-clippy-r530`: the original strict
  all-target Clippy command reproduced
  `clippy::case_sensitive_file_extension_comparisons` at
  `tests/live_rdp.rs:125`.
- `172.20.0.170`, slot `func016-live-rdp-clippy-r530`:
  `cargo clippy -p mde-vdi-rdp --all-targets --features live-connect -- -D warnings`
  passed.
- `172.20.0.170`, slot `func016-live-rdp-fmt-r530`:
  `rustfmt --edition 2021 --check crates/desktop/mde-vdi-rdp/tests/live_rdp.rs`
  passed.
- `172.20.0.196`, slot `func016-live-rdp-test-r530`:
  `cargo test -p mde-vdi-rdp --features live-connect --test live_rdp` passed
  8/8 executable tests; the environment-gated live endpoint test remained
  correctly ignored.

This removes a concrete compile-time gate blocker. It does not claim the
deferred post-release one-node clipboard/runtime acceptance proof.
