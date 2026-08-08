# WL-FUNC-021 Media source projection boundary — 2026-08-06

`mde-media-egui` now projects only unambiguous Jellyfin sources with safe
identities and bounded HTTP(S) endpoints. Duplicate source IDs are rejected as
a group, unsafe schemes and authority/path injection are refused, and hostile
source records cannot become selectable media clients.

Verification:

- BigBoy `.130`, slot `music-media-source-boundary-20260806-r2`:
  `cargo test -p mde-media-egui
  mesh_jellyfin_projection_rejects_ambiguous_identity_and_unsafe_endpoint --
  --nocapture` passed **1/1**.
- The initial `.50` attempt hit `ENOSPC`; no passing result is claimed for that
  host.
- `git diff --check` passed.
- Source SHA-256:
  `f4b4ea00fefb83857b9f3e70e2f4db5f4ea3bcfbbce56d284ed2081a12aa0e7f`.

This is source-level Media boundary proof only. Provider/audio recovery,
rendered acceptance, cast/handoff, package, and seat proof remain open. Dell
runtime was not modified.
