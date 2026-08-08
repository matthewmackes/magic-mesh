# WL-FUNC-021 Music artwork and bounded catalog pages — 2026-08-07

## Result

Music now treats provider artwork as a first-class, daemon-owned reference.
Airsonic `coverArt` tokens are retained with albums, artists, podcast channels,
episodes, radio stations, songs, bookmarks, recent/frequent rows, and search
groups. `get-cover-art` materializes admitted bytes in the node-local artwork
cache and returns a path; the UI decodes that path into textures on Home,
Library, album detail, bookmarks, queue, Continue Listening, now playing, and
the embedded discovery surfaces. Missing artwork uses a deterministic tile
fallback rather than an empty region. This implements the requested scope of
all media types and all Music surfaces.

## Farm gates

```text
BigBoy 172.20.0.130 / music-artwork-daemon-full-r1:
  mde-musicd: 199 passed, 0 failed

172.20.0.50 / music-artwork-ui-full-r1:
  mde-music-egui: 64 passed, 0 failed

172.20.0.90 / music-artwork-ui-fmt-r2:
  cargo fmt -p mde-music-egui -- --check: PASS

172.20.0.90 / music-artwork-shell-route-r2:
  typed browse-route regression: 1 passed, 0 failed
```

The native Fedora 44 release build completed on `.131`; the F44 builder was
halted afterward and canonical Fedora 42 builder `.130` was restarted.

## Artifact and deployment

```text
magic-mesh-12.1.6-6.x86_64.rpm
size: 87,591,150 bytes (83.5 MiB)
SHA-256: eb9d6194b6a03a835a4b533f124260a39afbdb8297d81da410fdedf45f6d225e
RPM media requirements: libavcodec.so.62, libswresample.so.6, libswscale.so.9
```

The exact artifact passed RPM payload and size gates, then passed an RPM test
transaction and was installed with `--replacepkgs` on both authorized targets:

```text
Dell 172.20.146.225: magic-mesh-12.1.6-6, mde-musicd active, mde-shell-egui active
Seat 15 172.20.0.15: magic-mesh-12.1.6-6, mde-musicd active, mde-shell-egui active
verify-music-live-seat.sh: PASS on both targets, NRestarts=0, rpm -V clean
```

## Live catalog and artwork proof

Both targets answered the typed catalog requests from `/run/mde-bus`:

| Request | Result on Dell and seat 15 |
| --- | --- |
| `list-albums` offset 0, size 100 | 100 rows, `has_more=true`, `coverArt=al-1701` |
| `list-albums` offset 100, size 100 | 100 different rows, first `Greatest Hits` / `al-824` |
| `list-albums` offset 1600, size 100 | 70 rows, `has_more=false`, first `System of a Down` / `al-1614` |
| `list-podcasts` | 31 channels, first `Wait Wait... Don't Tell Me!` / `pod-0` |
| `list-radio` | 53 stations |
| `get-cover-art` `al-824` | local JPEG path, 130,806 bytes, 700x700 |
| `get-cover-art` `pod-0` | local JPEG path, 84,261 bytes, 1400x1400 |

The album and podcast files were non-empty valid JPEGs on both seats. The
current C-SPAN Radio row is present, but Airsonic supplies no artwork token
for it; the UI therefore exercises its intentional fallback tile. Radio
artwork remains supported whenever the provider supplies it.

## Boundary

This proves daemon/API pagination and local artwork delivery for the deployed
release. It does not claim a new physical pixel capture of every UI surface,
provider-loss continuity, cast/renderer proof, cross-seat handoff, typed radio
playback, or the five-seat CPU/NWS acceptance boundary. WL-FUNC-021 remains
`Remaining` until those boundaries are separately evidenced.
