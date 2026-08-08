# WL-FUNC-016 — collaboration Clipboard V2 daemon intake (2026-08-05)

The live clipboard-sync worker now tails the strict collaboration
`ClipboardEnvelopeV2` Bus lane with a durable cursor and bounded per-source
replay/echo ledger. Signed, fresh, consented, exact-target inline `text/plain`
materializes through the existing seat handoff without writing legacy clipboard
history. Files references, rich MIME, explicit unsupported/unavailable states,
wrong targets, replay, and echoes fail closed; terminal refusals are acknowledged
without producing a materialization.

## Verification

- BigBoy `.130`, reused warm slot `wl-arch009-status-publish-r1` after the `.90`
  slot exhausted disk during link:
  `cargo test -p mackesd collab_v2_ -- --nocapture`.
- Result: `2 passed; 0 failed; 4428 filtered out` for the library; other test
  binaries had no matching tests.
- File-scoped Rust formatting passed. Files-backed binary/rich materialization,
  all-node fanout, KDC/mobile, RDP/SPICE, and live VDI proof remain open.
