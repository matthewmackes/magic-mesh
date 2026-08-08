# WL-FUNC-017 geocoder boundary — 2026-08-06

`crates/desktop/mde-maps-location-egui/src/geocode.rs` now rejects gazetteer
rows with non-finite or out-of-range coordinates, control characters, or
oversized display fields before they become navigation destinations.

Verification: BigBoy `.130`, slot `maps-geocode-hostile-boundary-20260806-r2`,
focused cargo test passed 1/1 (274 filtered); `git diff --check` passed.
Source SHA-256:
`2f44f21bfa0c46a2be7ce7b792715dd543ad3cc169dccda46200ff0b071f8b6f`.

Live GNSS/routing and five-seat Maps/MG90 proof remain open; Dell was not
modified.
