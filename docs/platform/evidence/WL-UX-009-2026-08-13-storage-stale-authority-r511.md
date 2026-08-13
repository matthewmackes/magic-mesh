# WL-UX-009 — Storage stale-mirror authority (r511)

Date: 2026-08-13
Scope: `crates/desktop/mde-shell-egui/src/storage/mod.rs`

## Result

The Storage surface no longer presents an arbitrarily old `Available` mirror
as live authority. After three missed 30-second worker heartbeats, the retained
topology is explicitly marked stale and remains visible only for orientation.
Peer and fleet status use the warning semantic, the Start Menu stops projecting
stale capacity as live, menu/device staging sees no authorizing disk, typed
Apply rechecks freshness, and any locally staged queue and arming echo are
discarded. Refresh remains reachable so the operator can recover authority.

The freshness clock is recorded only after a successful Bus projection. A
failed Bus open therefore cannot refresh retained state, and pure render
fixtures remain deterministic rather than depending on wall-clock time.

## Farm evidence

- BigBoy `172.20.0.130`, slot `ux009-storage-stale`:
  `cargo test -p mde-shell-egui stale_available_mirror_loses_storage_mutation_authority -- --nocapture`
  passed 1/1 with 1,589 filtered tests.
- `172.20.0.170`, slot `ux009-storage-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `172.20.0.50`, slot `ux009-storage-fmt`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- `git diff --check` passed.

No duplicate gate was launched. This is an implementation checkpoint, not
post-release visual acceptance. WL-UX-009 still requires the remaining surface
migration, first-release payload verification, and the deferred Dark/Light,
narrow/largest-text, stale/unavailable direct-DRM capture and human review.
