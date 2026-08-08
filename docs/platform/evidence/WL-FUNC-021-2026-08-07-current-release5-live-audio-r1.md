# WL-FUNC-021 current release-5 live Music/audio proof (2026-08-07)

This checkpoint binds the live Music service and bounded audio probe to the
current Fedora 44 release-5 package. It is evidence for the enrolled seats,
not a claim that the remaining provider-loss, handoff, renderer, or multi-seat
acceptance work is complete.

## Package and service provenance

The deployed artifact was `magic-mesh-12.1.6-5.x86_64.rpm` with SHA-256
`7ad3561f105c5e7f26440e8e7fc0828659db7b9b18bc3e23f71e06ac48d4aad8`. The
read-only live helper was run against the `mm` user service on both enrolled
targets:

```text
MUSIC_LIVE_HOST=172.20.0.15 MUSIC_LIVE_USER=mm ./install-helpers/verify-music-live-seat.sh
MUSIC_LIVE_HOST=172.20.146.225 MUSIC_LIVE_USER=mm ./install-helpers/verify-music-live-seat.sh
```

Both probes passed with `NRestarts=0`, the active service executing the
RPM-owned `/usr/bin/mde-musicd`, Bus `ping`, `action/music/get-state`, and
`action/music/list-albums`, payload identity, and `rpm -V magic-mesh`.
The helper self-test also passed. The probes were read-only; no provider
credential or audio content was printed or retained.

## Seat-15 PipeWire playback probe

On seat 15, the current release-5 `mde-musicd play 23427` action was bounded
while recording the active analog sink monitor as stereo 48 kHz signed
32-bit samples. Temporary capture files were removed by the remote trap.

```text
monitor=alsa_output.pci-0000_00_1f.3.analog-stereo.monitor
captured_bytes=8847360 samples=2211840 nonzero=1424381 peak=736320000
play_rc=124 record_rc=124
```

The capture contains a non-silent PipeWire signal from the current service
and provider track. Both return codes are the intentional bounded-probe
timeouts, not a claim that the track ended naturally. This proves current
package/service-to-PipeWire audio activity; it does not prove listener
speaker judgment, a full-track visual capture, provider-loss recovery, peer
handoff, a physical renderer, authenticated mutation/rotation, or the
five-seat CPU/NWS gate.

## Current provider-loss boundary

The observation-only loss helper was also run against seat 15 with a six
second window and one-second samples. It saw four consecutive healthy
samples (`service=active provider=ok catalog=ok state=ok`) and returned the
expected refusal:

```text
[REFUSAL] no natural provider loss was observed before the deadline
[REFUSAL] live provider-loss transition was not fully observed
probe_rc=3
```

No provider, interface, playback, or service state was changed. Live
provider-loss/recovery and audible continuity therefore remain open.
