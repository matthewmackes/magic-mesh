# WL-FUNC-024 leftover honesty — Seat 15 live Calls media — r1

Date: 2026-08-24  
Observed: `2026-08-24T16:25:16Z`–`2026-08-24T16:26:16Z`  
Classification: leftover-honesty / installed-seat live probe; **not**
two-seat audio, **not** chirp-correlation production proof, **not** LiveKit
SFU, **not** PSTN, **not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `91099e78a915`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH. No `seat-update-warning`. No call start. No `action/voip`
publish. No package install. No `systemctl` mutate. No dest invented.
Gateway / account files were probed for presence only; none existed to
read.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-024`.
- Leftover: live media / SFU / PSTN on qualification seats.
- Prior park: `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- In-tree S4 honesty: `WL-FUNC-024-2026-08-23-voice-hud-s4-pstn-drive-r1.md`
  (farm `64/64` `mde-voice-hud`; not live PSTN).
- Farm planes are not this leftover (`WL-FUNC-024-2026-08-20-media-s3-farm-r1.md`).
- PSTN consumes WL-FUNC-030 `gateway.toml`.

## Seat identity

| Field | Value |
|---|---|
| Seat | `172.20.0.15` `Basement-Test-Workstation` |
| SSH | `mm@` / `/root/.ssh/mackes_mesh_ed25519` |
| RPM | `magic-mesh-13.0.0-35.x86_64` (buildtime Sat 22 Aug 2026 09:56:45 PM EDT) |
| `mackesd --version` | `13.0.0 · 7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac · 2026-08-23 · dev` |
| Role | `/var/lib/mde/role.toml` `role = "workstation"` |
| Construct | `/usr/bin/mde-shell-egui` pid `476582` (started Mon Aug 24 11:50:06 2026 EDT), holds `/dev/dri/card1` |

Identical installed bytes vs the 2026-08-24 Files leftover probe:

```text
faef704f444727f165f964495ad9fec629674e2b6d0af23a13b7cbd265f08a14  /usr/bin/mde-shell-egui
b27a035167946e182d65f7784384b1b54c2418e63a515cb28fcb7127cbeb1009  /usr/bin/mackesd
```

Dell and Surface were not re-probed this unit.

## Media worker (data-group `collab`)

There is no separate `call_media` / `collab_media` systemd unit. The P2P,
LiveKit-SFU, and SIP publish planes are registered inside the rank-0
`collab` worker (`WorkerGroup::Data`).

`mackesd-data.service` ActiveEnterTimestamp `2026-08-24 12:00:10 EDT`
(`2026-08-24T16:00:10Z`). Journal:

```text
SIP Calls provider not activated  detail=no governed SIP account is installed  2026-08-24T16:00:10.570858Z
starting worker  worker=collab  2026-08-24T16:00:10.575743Z
```

Heartbeat topic `state/mackesd/data/workers/collab` is publishing
(generation advancing). Latest snapshot:

| Field | Value |
|---|---|
| `worker_id` | `collab` |
| `group` | `data` |
| `node_id` | `peer:Basement-Test-Workstation` |
| `state` | `running` |
| `generation` | `311` |
| `restart_count` | `0` |
| `state_reason` | none |
| `cadence` | `event_driven` |
| `state_since_ms` | `1787587210571` (= 16:00:10.571Z) |

Worker census is not a live call.

Sibling units at probe time: `mackesd-integrations`, `mackesd-actions`,
`mackesd-control`, `mackesd-compute`, `mackesd-observation`,
`mcnf-collaboration-identity`, `mde-shell-egui`, `syncthing` all `active`.
`mde-voice-hud`, `livekit`, and `livekit-server` units are `inactive`
(not installed).

`voice_provision` is running under integrations. That is Vitelity
sub-account provisioning, not the Calls media plane.

## LiveKit SFU

- `command -v livekit-server livekit lk` empty
- `rpm -ql magic-mesh` has no LiveKit / WebRTC / `mde-voice-hud` payload
  (voice names are Carbon icons only)
- no matching process
- installed `/usr/bin/mackesd` contains `LiveKitSfu` and
  `state/calls/media/` literals, `livekit-server` count 0, `chirp` count 0,
  `SharedLoopback` count 0

In-tree `LiveKitSfuPlane::production()` binds ALSA and leaves the
loopback/mixer seam unset so it cannot invent `Connected`. No elected SFU
host document exists on the bus (`/run/mde-bus/state/calls` absent).

## Chirp fixture

The epic's loopback/chirp seam is `#[cfg(test)]` `SharedLoopback` in
`crates/mesh/mackesd/src/workers/call_media.rs`. Production leaves
`loopback: None`. Seat find for `*chirp*` under `/etc`, `/var/lib`,
`/mnt/mesh-storage`, `/home/mm`, `/root`, `/usr/lib`, `/opt` returned
empty. No chirp process. Farm chirp-correlation fixtures are not a seat
fixture and cannot close this leftover.

## Bus: no live media session

| Topic | Observation |
|---|---|
| `state/collab/call-media-readiness` | one retained body `{"local_actor":"Basement-Test-Workstation","sessions":[]}` at 16:00:10.634Z |
| `state/collab/call-media-readiness/<space>` | same empty `sessions` for space `6f3eb1d3-a826-74bf-d462-9c02ad553488` |
| `state/collab/call-media-verification` | `{"local_actor":"Basement-Test-Workstation","rows":[]}` |
| `state/collab/call-state/<space>` | `{"active":[]}` |
| `state/calls` | **absent** (no `state/calls/media/<session>` /offer /answer /sfu /sip) |
| `/run/mde-bus/action` | **absent** |

`state/media/sources` is the Navidrome-era `media_sources` worker, not
Calls media.

`mde-bus history state/collab/call-media-readiness --bus-root /run/mde-bus`
repeats the empty-sessions document only.

## PSTN / FUNC-030 `gateway.toml`

Canonical workgroup path is `/mnt/mesh-storage/voip/gateway.toml`
(`default_workgroup_root()` + `voip/gateway.toml`). `/mnt/mesh-storage`
exists; the `voip/` directory does not. Also absent (paths only):

- `/etc/mackesd/gateway.toml`, `/etc/mackesd/voip/gateway.toml`
- `/var/lib/mackesd/gateway.toml`, `/var/lib/mde/gateway.toml` and `voip/`
- `~/.config/mcnf/gateway.toml` for `mm` and `root`
- `~/.config/mde/voice/account.toml` for `mm` and `root`
- `~/QNM-Shared/voip/gateway.toml` for `mm` and `root`

`find` for `gateway.toml` / `account.toml` returned empty. That matches
the 2026-08-22/23 fleet-negative and is the FUNC-030 leftover FUNC-024 S4
consumes.

The VOIP gateway Bus responder **did** spawn inside `mackesd-data` at
16:00:10.367Z (`serving action/voip/{set-gateway,get-gateway,clear-gateway}`).
No `action/voip` spool exists. Spawn is not a migrated `gateway.toml` and
not a PSTN leg.

Journal `no governed SIP account is installed` is the S4 fail-closed
activation path with that file absent.

## Audio / video hardware (capacity, not a call)

| Check | Result |
|---|---|
| `/dev/snd` | present (`controlC0`, analog PCM capture/playback) |
| `mm` groups | includes `audio` |
| PipeWire | `pipewire` + `pipewire-pulse` + `wireplumber` active (user) |
| `pactl` | PulseAudio on PipeWire 1.6.8; default sink/source `alsa_*pci-0000_00_1f.3.analog-stereo` |
| `/dev/video*` | **absent** |

Audio bind capacity exists. No live session used it. Video would be
device-absent if a camera track were offered.

## What this does not prove

- Two seats completing an audio call with objective tone/chirp correlation.
- Mute or DTMF acting on a live leg.
- A three-seat group call riding an elected LiveKit SFU, or honest
  SFU-degraded P2P fallback mid-call.
- Outbound or inbound PSTN through the LiveKit SIP gateway.
- `state/calls/media/<session>` Connected (or any session document).
- Camera or screen tracks.

Farm-green `mde-collab-types` / `mackesd` / `mde-collab-egui` /
`mde-voice-hud` remain implementation evidence only.

## Blocker

Live leftover stays open. Closing it needs a **current-revision**
unpublished candidate on at least two qualification seats (WL-REL-002),
then a real two-seat call with objective chirp/tone correlation, a group
SFU path with a real LiveKit host (none is installed here), and PSTN
only after FUNC-030 lands a migrated workgroup `gateway.toml` with a
governed provider — or an honest unavailable state if that file stays
absent. Seat 15 `collab` running with empty readiness is census.
Installed `13.0.0-35` is unpublished and older than HEAD. Do not invent
a dest. Do not flip `production_admitted`.
