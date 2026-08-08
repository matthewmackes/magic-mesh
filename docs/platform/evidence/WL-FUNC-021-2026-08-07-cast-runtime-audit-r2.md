# WL-FUNC-021 — cast/DLNA/Chromecast runtime audit r2 (2026-08-07)

Status: bounded read-only runtime discovery is now reproducible; physical
DLNA control, Chromecast CASTV2 control, mesh-owner casting, and live seat
handoff remain unproven. `WL-FUNC-021` remains `Remaining`.

## Scope and safety boundary

This audit changed only the owned helper and this evidence file. It did not
edit source, `docs/platform/WORKLIST.md`, or prior evidence. No package was
installed, no service was restarted, no seat was rebooted, and no physical
renderer control request, CASTV2 request, playback request, or handoff action
was sent.

## Bounded helper improvement

`install-helpers/verify-music-cast-loopback.sh` now has a separate
`--runtime-probe` mode. It performs exactly one read-only SSDP
`M-SEARCH` for `MediaRenderer` at `239.255.255.250:1900` and one read-only
mDNS PTR query for `_googlecast._tcp.local` at `224.0.0.251:5353`. The mDNS
socket joins UDP/5353 so multicast answers cannot be missed merely because a
diagnostic used an ephemeral source port. The probe listens for a bounded
3–15 seconds, caps stored records at 64 and datagrams at 8 KiB, decodes DNS
resource-record names/types, and reports `_googlecast` records separately.

The mode sends no SOAP action and no CASTV2 command. A completed probe with
zero answers is an observation, not physical renderer acceptance. The helper
also retains the disposable loopback HTTP exchange and its explicit
non-finite/malformed seek refusals.

Helper SHA-256 after this change:

```text
17cb44b4fb6b96306e39d2478648915f12d949cde3b79b5157fe2f205a5fb5fe  install-helpers/verify-music-cast-loopback.sh
```

## Verification

Local helper gates:

```text
bash -n install-helpers/verify-music-cast-loopback.sh
install-helpers/verify-music-cast-loopback.sh --self-test
verify-music-cast-loopback: self-test passed

shellcheck install-helpers/verify-music-cast-loopback.sh
git diff --check -- install-helpers/verify-music-cast-loopback.sh
```

All four commands passed. The loopback self-test still requires discovery,
device description, `SetAVTransportURI`, `Play`, finite `Seek`, and HTTP 400
refusal for malformed and non-finite seeks; it verifies listener/thread
cleanup.

The current cast contract suite ran on build-farm `.90` because BigBoy
`.130` was unavailable during this audit:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=music-cast-runtime-audit-20260807-r2 \
install-helpers/xcp-build.sh cargo test -p mde-media-core cast -- --nocapture

test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 230 filtered out
```

The suite covers bounded SSDP/mDNS projection, malformed target refusal,
ordered DLNA description → `SetAVTransportURI` → `Play` → `Seek`, truncated
success-response refusal, absent-renderer honesty, and typed Chromecast and
mesh gates. Current inspected source SHA-256:

```text
7fc7172e3ac83d8d4ef5de7298667240649183418405d85963acdcbe8cc0fa60  crates/desktop/mde-media-core/src/cast.rs
```

## Read-only runtime results

The helper was streamed over SSH to each authorized seat with
`MUSIC_CAST_RUNTIME_PROBE_SECONDS=3`. Both returned `status=completed`, no
probe errors, one SSDP query, one mDNS query, zero control requests, and no
mutations.

| Seat | Package | SSDP packets | mDNS packets | mDNS query packets | mDNS answer packets | `_googlecast` records |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| seat15 (`172.20.0.15`) | `12.1.6-4.x86_64` | 0 | 14 | 6 | 8 | 0 |
| Dell (`172.20.146.225`) | `12.1.6-5.x86_64` | 0 | 19 | 9 | 10 | 0 |

The mDNS answer packets were unrelated multicast DNS traffic: the bounded
decoder found no owner or RDATA containing `googlecast`. The answer count
therefore does not promote either seat to Chromecast discovery acceptance.
No SSDP `MediaRenderer` response was observed on either seat. Read-only
listener inspection found no local TCP `:8009` or UDP `:1900` listener; UDP
`:5353` listeners were present as mDNS responders, which is not a Chromecast
CASTV2 receiver.

Both seats had the expected runtime services healthy during inspection:

```text
seat15: mackesd active/running, NRestarts=0; user mde-musicd active/running, NRestarts=0
Dell:   mackesd active/running, NRestarts=0; user mde-musicd active/running, NRestarts=0
```

No live control path was exercised after these read-only checks.

## Source/runtime acceptance boundary

The inspected `crates/desktop/mde-media-core/src/cast.rs` contract remains
honest:

- DLNA/UPnP is a real live path, but requires an SSDP renderer response and a
  reachable device description exposing `AVTransport`.
- Chromecast discovery can project a target, but `NetworkCaster` returns a
  typed gate requiring the CASTV2 protobuf-over-TLS launch handshake on
  `:8009`; it does not fake a successful cast.
- Mesh-node casting returns a typed gate requiring a target-node
  cast-receiver worker.

Consequently, the current evidence proves the bounded implementation and the
absence of an advertised physical DLNA/Chromecast target on both inspected
seats. It does not prove renderer playback, Chromecast control, mesh-owner
yield, target resume, audible continuity, or a destination rendered frame.

## Blockers and next evidence

1. A reachable physical DLNA/UPnP `MediaRenderer` must answer SSDP and accept
   the complete description plus ordered control exchange.
2. A real Chromecast target and CASTV2 receiver implementation are required
   for Chromecast acceptance; current mDNS traffic contains no
   `_googlecast` records.
3. A mesh cast-receiver worker, two fresh admitted seats, and an authenticated
   idle target are required for live owner-yield/resume proof.
4. Destination-side audio and rendered-frame evidence is still required for
   continuity acceptance.

Conclusion: the runtime gap is now directly observable and fail-closed, but
the live cast/handoff acceptance boundary is unavailable on the current
network and seats. No claim of completion is made.
