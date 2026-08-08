# WL-FUNC-011 — daemon TransferJob V2 bridge (2026-08-05)

The authoritative daemon transfer ledger now exposes a bounded `project_v2`
adapter. A caller must supply an already-admitted typed transfer identity,
endpoint, and operation. The bridge projects only clean queued legacy rows;
running/terminal rows, inconsistent progress, legacy bandwidth tokens, and
method/kind mismatches fail closed. Legacy paths, URLs, commands, credentials,
and string IDs are never copied, and no `FileRefId` is minted.

## Verification

- Farm `.90`, slot `wl-func011-transfers-v2-r1`:
  `cargo test -p mackesd workers::transfers::v2 -- --nocapture`.
- Result: `5 passed; 0 failed; 4408 filtered out`.
- Exact transfer bridge files formatted on `.50`; `git diff --check` passed.
- The bridge does not claim complete protocol-lane parity, live transfer
  execution, or completion of WL-FUNC-011.
