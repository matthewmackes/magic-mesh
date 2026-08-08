# WL-FUNC-021 live provider audio capture (2026-08-06)

Seat 15 is the enrolled non-production Workstation at `172.20.0.15`.
This proof captures the PipeWire default sink monitor while the real provider
track `23427` is played. It proves a non-silent decoded/routed audio signal;
it does not claim a microphone measurement or a listener's physical-speaker
judgment.

## Command and bounds

The remote probe used `XDG_RUNTIME_DIR=/run/user/1000`, the default sink
monitor reported by `pactl`, `parec --format=s32le --rate=48000 --channels=2`,
and a 72-second timeout around:

```text
mde-musicd play 23427
```

The track metadata is recorded in
`WL-FUNC-021-2026-08-06-live-provider-audio-r1.md`; no provider credential or
audio content was printed or retained. Temporary capture/log files were
removed by the remote trap and verified absent afterward.

## Result

```text
PASS captured monitor audio: samples=6717440 nonzero=6287357 peak=861190400 mean_abs=107559166
PASS bounded real playback/audio capture: bytes=26869760 play_rc=0 record_rc=124
seat temp cleanup PASS
```

The playback command completed normally (`play_rc=0`). The recorder reached
its intentional 72-second bound (`record_rc=124`) after capturing a 26.8 MiB
stereo 48 kHz s32le stream. 6,287,357 of 6,717,440 samples were nonzero.

Open WL-FUNC-021 proof remains: provider/network-loss resume, peer handoff,
full rendered Music acceptance, and authenticated mutation delivery. The
companion direct-DRM and Music-specific artifact gate are recorded in
`WL-FUNC-021-2026-08-06-live-drm-frame-r1.md`.
