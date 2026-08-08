# WL-FUNC-021 cast and handoff runtime audit — 2026-08-06

Status: audit complete. This file records the current acceptance boundary; it
does not promote WL-FUNC-021, which remains `Remaining`.

## Scope and safety boundary

This was a read-only audit of the cast and peer-handoff boundary. No package
was installed, no service was restarted, no seat was rebooted, and no Dell or
seat runtime state was changed. No cast, transfer, play, or handoff mutation
was sent to a live seat. The only repository write from this audit is this
evidence file; `docs/platform/WORKLIST.md` and source files were not edited.

## Governing worklist state

The current `WL-FUNC-021` entry says that bounded cast proofs pass but live
renderer, Chromecast, mesh-owner, seat-handoff, live network-loss recovery,
full rendered acceptance, and installed-seat package proof remain open. The
existing loopback evidence makes the same limitation explicit. This audit
confirmed that boundary against the current source and live seats rather than
upgrading fixture results into live acceptance.

## What the current source proves

The inspected contracts are internally coherent and fail closed:

- `crates/desktop/mde-media-core/src/cast.rs` bounds discovery to 64 targets
  and one SSDP collection to 64 KiB. `SsdpProbe` sends a real
  `M-SEARCH` for `MediaRenderer` to `239.255.255.250:1900` and returns an empty
  discovery result when the probe has no replies.
- `NetworkCaster` performs real DLNA/UPnP device-description lookup followed by
  `SetAVTransportURI`, `Play`, and an optional finite `Seek`. Empty, negative,
  non-finite, and over-seven-day resume positions are rejected before network
  access.
- The same implementation returns typed `CastError::Gated` for Chromecast
  (`CASTV2` launch handshake still required) and mesh-node casting (a target
  mesh cast-receiver worker still required). It cannot report those paths as a
  successful cast.
- `crates/desktop/mde-music-egui/src/app.rs` only enables `Send` for an
  available `mesh_seat` with an authenticated Construct-shell publisher. Local
  and DLNA targets remain browse-only until their typed adapters exist.
- `crates/services/mde-musicd/src/bus_responder.rs` pauses the owner, writes the
  song and exact position, writes a completion, and clears the intent only
  after durable writes succeed. The target requires a queue match, an admitted
  source/cache candidate, and successful native-engine start at the saved
  position before it clears the completion. Peer targets are derived only from
  bounded peer heartbeats; stale or currently-owning peers are unavailable.

## Current automated evidence

Commands were run against the current dirty source through the build farm where
the test was more than a tiny local probe.

```text
bash -n install-helpers/verify-music-cast-loopback.sh && \
  install-helpers/verify-music-cast-loopback.sh --self-test
verify-music-cast-loopback: self-test passed
```

The current cast suite was run on BigBoy:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=music-cast-runtime-audit-20260806-r1 \
./install-helpers/xcp-build.sh cargo test -p mde-media-core cast -- --nocapture

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 222 filtered out
```

The current handoff suite was run on the farm:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=music-handoff-runtime-audit-20260806-r1 \
./install-helpers/xcp-build.sh cargo test -p mde-musicd handoff -- --nocapture

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 171 filtered out
```

These tests prove fixture/contract behavior only. They do not prove a live
renderer, cross-seat network delivery, audible continuity, or a rendered frame
on the destination seat.

## Read-only seat capability checks

The reusable read-only gate was run without its optional play probe:

```text
install-helpers/verify-music-live-seat.sh
== Music live seat verification (mm@172.20.0.15) ==
[OK] mde-musicd.service active (NRestarts=0)
[OK] mde-musicd ping answered
[OK] action/music/get-state answered on /run/mde-bus
[OK] action/music/list-albums answered on /run/mde-bus
[INFO] play probe disabled (pass --play-probe SONG_ID to enable)
verify-music-live-seat: PASS
```

The equivalent direct read-only check on Dell (`172.20.146.225`) also
reported the user `mde-musicd.service` active, `MainPID=1386`, `NRestarts=0`,
the Airsonic endpoint reachable at `http://172.20.0.2:4040` (API v1.15.0), and
`action/music/get-state` answered. Seat 15 reported `MainPID=1288`,
`NRestarts=0`, the same Airsonic endpoint, and an answered `get-state`.
Neither command sent a playback or transfer action.

## Physical renderer and Chromecast probes

From both seats, a bounded in-memory Python socket probe was run over SSH:

```text
timeout 12s ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes \
  -o ConnectTimeout=5 mm@<seat> python3 -
```

The probe sent exactly one SSDP request with `MX: 1`:

```text
M-SEARCH * HTTP/1.1
HOST: 239.255.255.250:1900
MAN: "ssdp:discover"
MX: 1
ST: urn:schemas-upnp-org:device:MediaRenderer:1
```

It listened for three seconds, then sent one mDNS PTR query for
`_googlecast._tcp.local` to `224.0.0.251:5353` and listened for three seconds.
Results:

```json
{
  "seat15": {
    "ssdp_media_renderer": {"packets": 0, "records": []},
    "mdns_googlecast": {
      "packets": 2,
      "records": [
        {"source": "172.20.0.15", "flags": "0x0", "qr": false, "questions": 1, "answers": 0},
        {"source": "172.20.146.225", "flags": "0x0", "qr": false, "questions": 1, "answers": 0}
      ],
      "answer_packets": []
    }
  },
  "dell": {
    "ssdp_media_renderer": {"packets": 0, "records": []},
    "mdns_googlecast": {
      "packets": 1,
      "records": [
        {"source": "172.20.146.225", "flags": "0x0", "qr": false, "questions": 1, "answers": 0}
      ],
      "answer_packets": []
    }
  }
}
```

The mDNS packets were query echoes (`QR=0`, zero answers), not Chromecast
service records. No physical DLNA/UPnP renderer answered either M-SEARCH. A
read-only `ss -lntup` sample also showed no local TCP `:8009` Chromecast
receiver or UDP `:1900` renderer endpoint. These are bounded observations on
the current network segment, not a claim that no renderer can ever exist.

## Current mesh-target and handoff state

The latest retained `state/music/workspace` record was read without changing
it. The safe projection (identity, kind, availability, and refusal reason
only) was:

```text
seat 15: revision=12964, workspace_files=151, target_count=0, targets=[]
Dell:    revision=3269,  workspace_files=29,  target_count=0, targets=[]
```

Thus neither current workspace exposes a local or remote target that could be
selected for a live transfer. This is consistent with the source contract:
the target projection does not manufacture a renderer when no local audio or
fresh peer-heartbeat target is admitted.

## Findings

| Boundary | Current result | Acceptance classification |
| --- | --- | --- |
| DLNA discovery/control | SSDP returned zero physical renderers; source and fixture path are green | Fixture/source proven; live renderer unavailable |
| Chromecast discovery/control | No mDNS answer records; CASTV2 path is typed `Gated` | Not proven; infrastructure unavailable |
| Mesh-node cast receiver | Source returns typed `Gated`; no receiver target was projected | Not proven; receiver worker unavailable |
| Owner yield | Farm contract tests pass; no live transfer was sent | Not proven live |
| Target resume | Farm contract tests pass; no live target/source/engine continuity was exercised | Not proven live |
| Audio/video continuity | Existing seat audio/DRM evidence is local playback only | Does not prove cast/handoff continuity |
| Dell/seat package acceptance | Both seats expose the installed `12.1.6-4` runtime, but current-source package/handoff proof is not established | Open |

## Named blockers and required next evidence

1. A reachable physical DLNA/UPnP MediaRenderer must answer SSDP and accept
   the complete description → `SetAVTransportURI` → `Play` → finite `Seek`
   exchange.
2. A Chromecast target and a CASTV2/receiver implementation are required for
   Chromecast acceptance; the current source intentionally reports this as a
   gate.
3. At least two live mesh seats with fresh heartbeats, an idle target, an
   admitted source/cache path, and authenticated action delivery are required
   to prove owner yield, target resume, and position continuity.
4. Destination-side PipeWire/audio evidence, a nonblank rendered frame, and
   operator-reviewed continuity evidence are required; local playback evidence
   cannot substitute for this.
5. The current source must be installed or otherwise exercised through a
   controlled live/package path before claiming installed-seat authorization
   or rotation acceptance.

Conclusion: the cast and handoff contracts are bounded, tested, and honest,
but the requested live cast/handoff acceptance boundary is unavailable on the
current seats and network. WL-FUNC-021 must remain `Remaining`.
