# WL-FUNC-021 live-seat verification gate (2026-08-06)

This is a bounded, read-only seat gate. It does not claim audible capture,
nonblank DRM frames, network-loss recovery, handoff, or authenticated
mutation acceptance.

## Implementation

`install-helpers/verify-music-live-seat.sh` checks the canonical seat through
bounded SSH commands. The default path checks:

- `mde-musicd.service` is active and reports `NRestarts=0`;
- `mde-musicd ping --retry 0` answers;
- `action/music/get-state` answers on `/run/mde-bus`; and
- `action/music/list-albums` answers on `/run/mde-bus`.

The play probe is disabled by default. `--play-probe SONG_ID` is explicit,
bounded to 30 seconds, and checks that the client process does not remain.
The helper never prints provider credentials or Bus reply bodies.

## Checks

```text
bash -n install-helpers/verify-music-live-seat.sh                         PASS
install-helpers/verify-music-live-seat.sh --self-test                     PASS
install-helpers/verify-music-live-seat.sh                                  PASS
```

Seat-15 default run:

```text
[OK] mde-musicd.service active (NRestarts=0)
[OK] mde-musicd ping answered
[OK] action/music/get-state answered on /run/mde-bus
[OK] action/music/list-albums answered on /run/mde-bus
[INFO] play probe disabled (pass --play-probe SONG_ID to enable)
verify-music-live-seat: PASS
```

The explicit bounded probe was also run against the live song `23427`:

```text
[OK] mde-musicd.service active (NRestarts=0)
[OK] mde-musicd ping answered
[OK] action/music/get-state answered on /run/mde-bus
[OK] action/music/list-albums answered on /run/mde-bus
[OK] explicit play probe bounded at 15s (rc=124)
verify-music-live-seat: PASS
```

The timeout is intentional. The helper confirmed that no `mde-musicd play`
client process remained afterward; PipeWire graph observations for this
stream are recorded in the live provider/audio evidence.

The live provider/audio and authorization findings are recorded separately in
`WL-FUNC-021-2026-08-06-live-provider-audio-r1.md` and
`WL-FUNC-021-2026-08-06-auth-delivery-audit-r1.md`.
