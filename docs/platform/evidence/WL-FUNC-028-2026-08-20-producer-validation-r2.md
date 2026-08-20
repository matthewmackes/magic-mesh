# WL-FUNC-028 recurring-mirror producer validation

- Date: 2026-08-20
- Scope: CLI `transfer sync-pair add` and the Communications Transfers
  editor/shell inbox route.
- Correctness gap closed: malformed rsync `bwlimit` tokens were accepted by
  producers, written as queued `SaveSyncPair` records, and only rejected later
  by the rsync lane. The GUI route also lacked the daemon's NUL/path and
  positive-interval boundary checks.
- Implementation:
  - `mackesd` CLI rejects invalid `bwlimit` values before writing the inbox.
  - Transfers editor reports NUL-bearing source/destination and malformed
    `bwlimit` input locally.
  - `mde-shell-egui` revalidates the typed command at the final inbox boundary,
    refusing invalid IDs, empty/NUL paths, zero intervals, and invalid
    `bwlimit` values; rejected commands leave no inbox record.
- Focused tests added:
  - `mackesd` CLI producer invalid-input coverage includes hostile `bwlimit`.
  - `mde-shell-egui` verifies worker-droppable GUI commands are refused before
    inbox publication.
- Farm gate attempt: `cargo test -p mackesd sync_pair -- --nocapture` on
  BigBoy and `cargo test -p mde-shell-egui sync_pair -- --nocapture` on `.50`
  were refused before sync because both selected lanes had less than the
  required 8 GiB `/home` headroom. The farm was otherwise 8/10 heavy slots
  active; no local heavy test was substituted.
- Limits: static/focused farm evidence only; no installed-seat, live-peer, or
  production release evidence is claimed.
