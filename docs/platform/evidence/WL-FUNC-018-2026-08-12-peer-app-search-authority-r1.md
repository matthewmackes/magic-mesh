# WL-FUNC-018 peer-app search authority — 2026-08-12

## Scope

`crates/desktop/mde-shell-egui/src/front_door.rs`.

Peer-app search rows on one serving peer previously shared the same
`SearchItem.target`. The ranked-search authority fold therefore treated two
different Flatpak app IDs as an equivocation and removed both rows. The target
now includes the typed Flatpak ID, and the app name is an explicit searchable
term. Favorites remain typed peer-app identities and do not become host
surfaces.

## Farm gates

- `.90`, `frontdoor-favorite-regression`: focused regression passed 1/1:
  `cargo test -p mde-shell-egui --bin mde-shell-egui front_door::tests::peer_app_favorites_round_trip_and_rank_without_host_surface_conversion -- --nocapture`.
- `.170`, `frontdoor-build`: `cargo build -p mde-shell-egui` passed; dev profile
  finished in 6m54s.
- `.170`, `frontdoor-clippy-final`: `cargo clippy -p mde-shell-egui --bin
  mde-shell-egui` passed with existing warnings (1339 warnings); no errors.
- Earlier `.90` full shell baseline: 1569 passed, 9 failed. The direct
  WL-FUNC-018 favorite failure is fixed by this slice; the other failures were
  unrelated shell baselines and remain outside this scope.

## Remaining acceptance

Signed catalog trust provisioning, App-VM image/live boot, VDI readiness,
security/package proof, and live UX/reconnect evidence remain open for
WL-FUNC-018. Live proof requirements are deferred per the current execution
direction.
