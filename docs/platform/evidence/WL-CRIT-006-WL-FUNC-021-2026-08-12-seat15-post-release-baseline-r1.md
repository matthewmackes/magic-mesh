# Seat 15 post-release baseline — 2026-08-12

## Selected baseline

Per the operator authorization to select any seat, Basement seat 15
(`172.20.0.15`, `Basement-Test-Workstation`) was selected as the single
post-release baseline seat. No other physical seat was exercised.

## Exact bounded gate

```text
MUSIC_LIVE_HOST=172.20.0.15 MUSIC_LIVE_SSH_KEY=/root/.ssh/mackes_mesh_ed25519 install-helpers/verify-music-live-seat.sh
```

Result: `verify-music-live-seat: PASS`.

Observed checks:

- `mde-musicd.service` active with `NRestarts=0`;
- active daemon executes the RPM-owned `/usr/bin/mde-musicd`;
- mde-musicd ping, `action/music/get-state`, and
  `action/music/list-albums` answered over `/run/mde-bus`;
- installed `magic-mesh` payload includes `mde-musicd` and `mde-shell-egui`;
- `rpm -V magic-mesh` reported no installed-file differences;
- installed `magic-mesh-12.1.6-33.x86_64` matched declared platform/RPM
  release `12.1.6/33` and verified both payloads.

## Bounded playback probe

After catalog discovery returned Warrant album `1701`, song `23427` (“Blood
Moon Prelude”), the same bounded gate was rerun with:

```text
MUSIC_LIVE_HOST=172.20.0.15 MUSIC_LIVE_SSH_KEY=/root/.ssh/mackes_mesh_ed25519 MUSIC_LIVE_PLAY_TIMEOUT_SECONDS=15 install-helpers/verify-music-live-seat.sh --play-probe 23427
```

Result: `verify-music-live-seat: PASS`. The explicit play probe reached its
15-second bound with the accepted timeout result (`rc=124`) and left no client
process; service, Bus, and RPM checks passed again. This proves bounded daemon
playback initiation and cleanup, but does not claim audible/rendered playback
without a direct audio capture.
