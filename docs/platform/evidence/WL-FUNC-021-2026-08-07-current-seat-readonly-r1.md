# WL-FUNC-021 — current-seat read-only acceptance sweep (2026-08-07)

## Scope

This was a bounded, read-only sweep using
`install-helpers/verify-music-live-seat.sh` against the current Fedora 44 seat
inventory. No package, service, playback, network, or DRM state was changed.
The helper was bound to the current source declaration `12.1.6-5`.

## Results

- Basement seat 15 (`172.20.0.15`): `mde-musicd.service` active with
  `NRestarts=0`; ping, `action/music/get-state`, and `list-albums` answered.
  Payload and RPM verification passed, but the installed package is
  `magic-mesh-12.1.6-4.x86_64`, so current-package proof correctly refused.
- Eagle (`172.20.146.88`): service, ping, state, and albums answered; payload
  and RPM verification passed, but the installed package is
  `magic-mesh-12.1.6-2.x86_64`, so current-package proof correctly refused.
- T480 (`172.20.146.68`) and Microsoft Surface (`172.20.146.79`): SSH was
  refused for the authorized key/account; no seat state was changed or claimed.
- Dell (`172.20.0.225`): `No route to host`; the known overlay endpoints
  `10.42.0.4` and `10.42.0.146` timed out. No deployment was attempted.

The helper self-test and the loopback provider-loss helper self-test passed.
These observations strengthen the live boundary but do not prove current
package, five-seat CPU, provider-loss, renderer, or two-seat handoff
acceptance. WL-FUNC-021 remains `Remaining`.
