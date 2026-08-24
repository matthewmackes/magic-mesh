# WL-FUNC-024 S4 — voice-hud gateway source honesty — r1

Date: 2026-08-24
Classification: in-tree implementation + focused farm gate; **not** live
PSTN / LiveKit SIP / installed-seat production proof
Unit: `03c5a4b2bb89` (`cargo test -p mde-voice-hud`)
Write scope: `crates/services/mde-voice-hud/src/sip.rs` only

## Deliverable

A present workgroup `gateway.toml` is the governed PSTN source and must not
silently fall through to a node-local `account.toml`:

- `SipAccount::accounts_from_sources` is the pure resolver. `gateway =
  Some(…)` always wins: parse success yields that account; empty / malformed
  / username-less bytes yield `None`. Only `gateway = None` (file absent)
  consults the node-local bytes.
- `load_accounts` maps a missing file to the local path, and maps a present
  but unreadable file (permission / I/O) to fail-closed `None`.
- `plan_pstn_agent` treats a whitespace-only inbound password as empty
  (`ABSENT_PSTN_PROVIDER`), matching the empty-secret fail-close from
  `WL-FUNC-024-2026-08-23-voice-hud-s4-pstn-drive-r1.md`.

Live two-seat / provider PSTN remains the parked leftover
(`WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`,
`WL-FUNC-024-2026-08-24-live-media-r1.md`); this crate cannot mint that
proof. PSTN still depends on WL-FUNC-030 landing a migrated
`gateway.toml`.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-voice-hud
Admission: 29125196 KiB free (required 8388608 KiB)
Result: 65 passed, 0 failed (unittests); 0 doc-tests
Elapsed compile+test: ~2m 08s compile, 0.44s tests
```

New focused case: present/malformed/empty gateway never consumes a valid
local credential. Whitespace-only secret stays Unavailable. Local
`cargo fmt -p mde-voice-hud` only.

## Leftover

A governed provider completing a live PSTN leg still depends on WL-FUNC-030
gateway.toml on a current-revision seat and the WL-REL-002 unpublished
candidate + red alert + 5s seat-mutation lock. Farm-green crate tests are
not that proof.
