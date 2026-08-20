# WL-FUNC-028 recurring-mirror producer seam

- Date: 2026-08-20
- Scope: `mackesd transfer sync-pair add|remove|list` and the
  Communications Transfers editor's typed `SaveSyncPair` /
  `RemoveSyncPair` inbox seam.
- Implementation:
  - `crates/mesh/mackesd/src/cli/transfer.rs` now validates interval,
    source, destination, NUL bytes, and explicit pair IDs before writing an
    inbox verb. Invalid producer input cannot report queued and then disappear
    during daemon validation.
  - Existing `SyncPairCmd::Add` is the create/edit (replace-by-ID) producer;
    `Remove` performs an early unknown-ID refusal; `List` reads the durable
    `SyncPairStore`.
  - Communications continues to emit the same typed wire verbs and folds
    worker-owned records for next-run, last-result, and reachability display.
- Focused farm admission: BigBoy `172.20.0.130`, slot `1`; 28,500,624 KiB
  free before sync.
- Command:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 install-helpers/xcp-build.sh
  cargo test -p mackesd sync_pair -- --nocapture`
- Result: exit 0; daemon/store and scheduler coverage 8 passed, CLI producer
  coverage 6 passed, 0 failed.
- Limits: this is static/focused farm evidence. No installed-seat, live-peer,
  or production release evidence is claimed.
