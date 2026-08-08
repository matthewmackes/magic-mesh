# WL-FUNC-011 — daemon TransferJob V2 ledger and inbox (2026-08-05)

The running transfers worker now drains strict V2 submit/control commands into
a separate node-local durable ledger. It admits only valid queued jobs, preserves
opaque Files-issued identities, rejects duplicates, stale/replayed control
timestamps, and illegal transitions, and keeps V2 jobs out of the legacy
path/URL executor until a typed Files resolver exists. Inbox reads are bounded,
regular-file-only, and final-symlink-safe; hostile records are consumed without
replay.

## Verification

- Farm `.90`, slot `wl-func011-v2-ledger-r1`:
  `cargo test -p mackesd --lib workers::transfers:: -- --nocapture`.
- Result: `75 passed; 0 failed; 4355 filtered out`.
- Full V2 endpoint resolution, protocol execution, byte-progress updates,
  signed global summaries, and live transfer proof remain open.
