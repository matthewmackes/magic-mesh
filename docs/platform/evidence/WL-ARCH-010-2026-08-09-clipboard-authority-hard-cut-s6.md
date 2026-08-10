# WL-ARCH-010 S6 clipboard authority hard cut

Date: 2026-08-09

## Result

The obsolete daemon clipboard relay is absent from the worker module tree,
production spawn surface, canonical worker registry, and privileged Bus
consumer inventory. `clipboard_sync` folds canonical VNC-attributed
`event/clipboard/clip` records into replicated history without publishing a
secondary action. The production shell remains the sole VDI clipboard
authority through lease-bound V2 messages and the live RDP CLIPRDR, VNC RFB cut
text, and SPICE vdagent protocol loops.

The active parity ledger now records those live V2/protocol lanes as current
state and does not prescribe restoration of the retired daemon authority.
Archives and prior evidence were not rewritten.

## Focused farm verification

- `172.20.0.130`, slot `arch010-s6-clipboard-cut`:
  `cargo test -p mackesd --features async-services --lib workers::clipboard_sync::tests -- --nocapture`
  — 39 passed, 0 failed.
- `172.20.0.130`, same slot after final synchronization:
  `cargo test -p mackesd --features async-services --lib workers::clipboard_sync::tests::vnc_clip_event_folds_canonical_history_without_secondary_action -- --nocapture`
  — 1 passed, 0 failed.
- `172.20.0.50`, slot `arch010-s6-worker-census`:
  `cargo test -p mackesd --lib worker_role::tests -- --nocapture`
  — 29 passed, 0 failed. This includes the registry hash, spawn/census drift,
  and retired clipboard source-symbol guard.
- `172.20.0.90`, slot `arch010-s6-vdi-clipboard`:
  `cargo test -p mde-shell-egui --features live-vdi,drm vdi::tests:: -- --nocapture`
  — 64 passed, 0 failed, 2 live-console hardware smokes ignored. The green set
  includes truthful backend support, canonical session attribution, rich
  fallback/reconnect deduplication, and secret/oversize/expiry refusal.
- `172.20.0.170`, slot `arch010-s6-fmt`: final synchronized
  `clipboard_sync.rs` passed `rustfmt --edition 2021 --config skip_children=true --check`.

## Boundary retained

- `ClipboardEnvelopeV2`, `VdiClipboardMessageV2`, per-session lease,
  generation, sequence, expiry, permission, receipt, replay, and reconnect
  contracts remain intact.
- RDP, VNC, and SPICE protocol writes remain owned by the live shell loops;
  receipts still follow successful transport effects.
- Guest clipboard events continue into canonical history.
- No compatibility consumer or second action authority was added.
