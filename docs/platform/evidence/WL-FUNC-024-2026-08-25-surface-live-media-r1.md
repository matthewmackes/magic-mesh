# WL-FUNC-024 leftover honesty — Surface live Calls media — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:30Z`–`2026-08-25T10:52:52Z`  
Classification: leftover-honesty / installed-seat live probe; **not**
two-seat audio, **not** chirp-correlation production proof, **not** LiveKit
SFU, **not** PSTN, **not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `4071ed295e18`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

Read-only SSH. No `seat-update-warning`. No call start. No `action/voip`
publish. No package install. No `systemctl` mutate. No dest invented.
Seat 15 and Dell were not SSHed (parent/Health Fix). Gateway / account
files were probed for presence only; none existed to read.

In-tree S1–S6 UI/types and S4 `sip.rs` were not changed this unit. Source
in `calls.rs` / `mde-collab-types` / `mde-voice-hud` `sip.rs` already
fail-closes mute/DTMF without a published session and maps an absent
provider to typed unavailable. Farm gates were not re-run.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-024`.
- Prior Seat 15 census (older SHA): `WL-FUNC-024-2026-08-24-live-media-r1.md`.
- Dest-cut: `WL-FUNC-023-2026-08-24-destcut-4071ed295-upgrade-wipe-r1.md`.
- Identity dest (receipt still pinned to the previous SHA):
  `WL-FUNC-023-2026-08-24-surface-collab-dest-admitted-r1.md`.
- PSTN consumes WL-FUNC-030 `gateway.toml`.

## Seat identity

| Field | Value |
|---|---|
| Seat | `172.20.146.79` `SURFACE` |
| SSH | `root@` / `/root/.ssh/mackes_mesh_ed25519` |
| Overlay | `10.42.0.7` (`/var/lib/mackesd/nebula/overlay-ip`, 10 bytes) |
| RPM | `magic-mesh-13.0.0-35.x86_64` |
| `mackesd --version` | `13.0.0 · 4071ed295e18a8bd117cea5ee639eb5cafab3485 · 2026-08-24 · dev` |
| Role | `/var/lib/mde/role.toml` `role = "workstation"` |
| Construct | `/usr/bin/mde-shell-egui` pid `1290917` (started Mon Aug 24 20:25:56 2026 EDT), holds `/dev/dri/card1` |
| `rpm -V` `mde-shell-egui` / `mackesd` | unmodified |

```text
2658ea3f142750646b98798e18bb35dad6e35b3eed8c3a5cf592193e32d2fe91  /usr/bin/mde-shell-egui
43cec1fb621e0e9fb9e0015ac82f1341d15ab0a1a2f0e0ebaf0987ebf4cbea54  /usr/bin/mackesd
```

Nebula `active`. Overlay ping from this seat: LH1 `10.42.0.1` 14.8 ms,
Dell `10.42.0.4` 1.0 ms, Seat 15 `10.42.0.5` 0.8 ms. Overlay ICMP is not
a call.

## Collab / media worker did not spawn

`mackesd-data.service` ActiveEnterTimestamp `2026-08-25 06:00:12 EDT`
(MainPID `3247869`). Journal at start:

```text
collab worker: governed collaboration identity unavailable; not spawning
error=collaboration identity admission is stale, malformed, or out of scope
2026-08-25T10:00:12.526096Z
```

Same refuse for `chat`. `/run/mde-bus/state/mackesd/data/workers/collab`
is absent. Worker census on this seat is "not spawned", not a live call.

Non-secret admission fields (receipt + materializer copy):

| Field | Value |
|---|---|
| `source_revision` | `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac` |
| installed `mackesd` | `4071ed295e18a8bd117cea5ee639eb5cafab3485` |
| `target_node` | `peer:SURFACE` |
| `mcnf-collaboration-identity` | `active` / `exited` since `2026-08-24 14:04:14 EDT` (before dest-cut) |

`source_revision` still names the pre-dest-cut binary. That is FUNC-023
identity leftover after `4071ed295` dest-cut, not a `calls.rs` gap. This
unit did not re-dest identity.

`mackesd-actions` did spawn the VOIP gateway responder at
`2026-08-25T10:00:12.244558Z`. `/run/mde-bus/action` is still absent.
Spawn is not a PSTN leg.

`mde-voice-hud`, `livekit`, and `livekit-server` units are `inactive`.
No matching process. Installed `mackesd` contains `LiveKitSfu` (count 1)
and `state/calls/media/` (count 1); `livekit-server` count 0,
`SharedLoopback` count 0, `chirp` paths empty.

## Bus: no live media session

| Topic / path | Observation |
|---|---|
| `state/collab/call-media-readiness` | no history |
| `state/collab/call-media-verification` | no history |
| `state/calls/media/<session>` | **absent** (`/run/mde-bus/state/calls` absent) |
| `/run/mde-bus/action` | **absent** |

`state/media/sources` is the Navidrome-era `media_sources` worker, not
Calls media.

## PSTN / FUNC-030 `gateway.toml`

Canonical `/mnt/mesh-storage/voip/gateway.toml` absent
(`/mnt/mesh-storage/voip/` does not exist). Also absent (paths only):
`/etc/mackesd/gateway.toml`, `/var/lib/mackesd/gateway.toml`,
`/var/lib/mde/gateway.toml`, `~/.config/mcnf/gateway.toml` and
`~/.config/mde/voice/account.toml` for `root` and `mm`. `find` for
`gateway.toml` / `account.toml` returned empty.

## Packed Calls literals (Construct, not a live leg)

Exact byte counts in `/usr/bin/mde-shell-egui`:

| Literal | Count |
|---|---|
| `SetCallMuted` | 2 |
| `SendDtmf` | 2 |
| `state/calls/media/` | 2 |
| `Unavailable: no live media provider` | 3 |
| `Unavailable: microphone permission denied` | 1 |
| `VoiceAccounts inbound register` | 1 |

Those strings prove the dest-cut UI is on the seat. They are not mute or
DTMF on a live sender.

## Audio / video hardware (capacity, not a call)

| Check | Result |
|---|---|
| `/dev/snd` | present (`controlC0` + `controlC1`, analog PCM) |
| `mm` groups | includes `audio` and `video` |
| PipeWire | `pipewire` + `pipewire-pulse` + `wireplumber` active (user) |
| `pactl` | PulseAudio on PipeWire 1.6.8; sink `alsa_output.usb-Microsoft_Corp._Microsoft_Docking_Station_Audio_Device_00000000-00.analog-stereo`; source `alsa_input.pci-0000_00_1f.3.analog-stereo` |
| `/dev/video*` | **present** (`video0`/`video1` names `ipu3-cio2 0/1`) |

Audio and camera bind capacity exists. No live session used it. Construct
journal since dest-cut has no `open_surface` / Calls lines.

## What this does not prove

- Two seats completing an audio call with objective tone/chirp correlation.
- Mute or DTMF acting on a live leg.
- A group call riding an elected LiveKit SFU.
- Outbound or inbound PSTN through the LiveKit SIP gateway.
- `state/calls/media/<session>` Connected (or any session document).

## Blocker

Live leftover stays open. Surface is on the current dest-cut SHA, but
`collab` (the media-plane host) did not spawn because the collaboration
identity receipt is still pinned to `7e3474eeb`. Two-seat audio also
needs Seat 15/Dell (parent Health Fix; overlay-ip empty there on dest-cut
record) and was not attempted. PSTN still depends on FUNC-030
`gateway.toml`. Do not invent a dest. Do not flip `production_admitted`.
