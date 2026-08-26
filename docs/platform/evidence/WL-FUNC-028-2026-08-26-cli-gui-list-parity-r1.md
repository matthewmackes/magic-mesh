# WL-FUNC-028 S1/S2 — CLI/GUI list+trim parity — r1

Date: 2026-08-26  
Classification: source-only CLI/GUI producer parity; **not** live seat
paint/click, **not** a pair add, **not** `production_admitted`  
Source revision: this fold on `agent/drain-worklist-20260725`  
Farm: worker `172.20.0.90` slot 1 (`magic-mesh-farm-1`)
`xcp-build.sh cargo test -p mde-collab-egui` → **188 passed, 0 failed**
(exit 0, 102405 ms). Did not run `cargo test -p mackesd` (dirty mackesd
files from a peer agent).

## Slice

Remaining source-only CLI/GUI producer gaps, without occupying Surface /
Dell / Seat 15 and without inventing dests:

- `mackesd transfer sync-pair list` human table now mirrors the Transfers
  editor: compact interval, worker `next_run_ms` (`next-run pending` /
  `due now` / `next in …`), worker `last_result` (`never run` / `last:
  …`), and `unreachable` when `peer_reachable == false`. `--json` is
  unchanged (full store records).
- `sync-pair add` trims id/source/destination/bwlimit before validate and
  slug, matching the editor so identical requests post identical inbox
  payloads (not only after daemon `normalize_for_save`).
- Editor tests cover the remaining CLI refuse cases (invalid id, empty
  source, NUL, hostile bwlimit), empty-id slug, and trim.

Worker engine, inbox, and `SyncPairStore` are unchanged. No second store
or scheduler.

## Non-claims

This is not live Construct Transfers paint/click. No inbox was created,
no pair was added on a seat, Sunshine was not started, and Ctrl+J was
not injected. Leftover remains `@leftover:{live-seat}`.
`production_admitted` was not flipped.
