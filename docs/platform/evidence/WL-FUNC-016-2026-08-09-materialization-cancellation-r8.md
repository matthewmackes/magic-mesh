# WL-FUNC-016 materialization cancellation boundary (r8)

- Date: 2026-08-09
- Base commit: `70b950d29d4d789234a2d95a76f902709a74a435`
- Production source: `crates/desktop/mde-shell-egui/src/clipboard_permissions.rs`
- Source SHA-256: `d450fb09e9395baa806491d3dcff653614f46e15b2d0c99fb5602a95a0101e0f`

## Correction

Clipboard cancellation previously became terminal in the shell model but did
not revoke a transport ticket after its one-use transition to materializing.
Operator cancel, expiry, focus loss, or session/lease replacement could
therefore remain invisible to a multi-step VDI protocol write. Refusal now
overrides the materializing atomic state while approval remains unable to
rewind it. The transport can observe revocation and stop before publishing more
bytes.

## BigBoy proof

Host `172.20.0.130`, slot `func016-r4-20260809`:

- Exact-file `rustfmt --edition 2021 --check`: passed.
- Hostile post-materialization cancellation regression: 1 passed, 0 failed.
- Complete `clipboard_permissions::tests` slice: 13 passed, 0 failed.
- Scoped `git diff --check`: passed.

The first shell test compile encountered five unrelated missing test imports in
`toast_bridge.rs`. A farm-only import patch unblocked the existing test target;
it was not applied to the production worktree and is not evidence for this
correction.

## Remaining limitation

This proves the production permission/ticket boundary deterministically, not
protocol-level interruption during a live RDP, VNC, or SPICE guest transfer.
Rich non-text materialization, shared CAS cleanup, and five-seat live
DRM/mesh/VDI evidence remain open.
