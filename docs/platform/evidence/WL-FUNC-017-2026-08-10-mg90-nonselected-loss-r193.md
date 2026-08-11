# WL-FUNC-017 MG90 non-selected manager loss — r193

- Scope: `VehicleRoster::mark_unavailable` now preserves the selected source
  publication epoch when an authoritative failure removes a non-selected
  manager row. The source remains eligible through the live manager and does
  not emit a false `Changed` publication.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func017-mark-unavailable-epoch-r193 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::vehicle::tests::marking_non_selected_manager_unavailable_preserves_source_publication_epoch -- --exact --nocapture`.
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4726 filtered out` on
  seat `.90`.
- Live limits: no physical MG90 manager-loss/reconnect trace, multi-seat
  hardware proof, or production Maps/Car publication capture was available;
  this checkpoint is limited to the deterministic roster safety boundary.
