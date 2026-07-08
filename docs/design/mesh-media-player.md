# mde-media — a VLC-class media player + Jellyfin client, native to the mesh

*Operator directive 2026-07-02: "VLC Media Player with Jellyfin Support, made native
to the platform" + "sessions should move between nodes if operator changes seats" +
"the client should list all services on the mesh that are serving media, or are
jellyfin servers." Locked via a 35-question survey.*

A new egui-native universal player (`mde-media-egui`) driven by **libmpv** — plays
virtually any audio/video, local or streamed — with a full **Jellyfin** client, and
extended for the mesh: auto-discovered media sources, an aggregated mesh library,
**session roaming** (your playback follows you between seats), sync-play party mode,
casting, and this node acting as a media server too.

## Locks (35-Q survey)

| # | Question | Decision |
|---|---|---|
| 1 | Engine | **libmpv** — embed mpv (render API); every format via system ffmpeg/codecs, HW decode, subs, streaming. System `libmpv` (dnf) + a Rust binding added to the lock + vendored. §6 glue. |
| 2 | Surface | **New `mde-media-egui` crate** (sibling of the other -egui surfaces). |
| 3 | Music app | **New universal A+V player; `mde-music`/`mde-musicd` stays** the music-library app (coexist). |
| 4 | Video path | **Dedicated DRM overlay plane** — scan out video on a HW plane beneath egui (best power/latency; composited with the egui-owned DRM). |
| 5 | Audio out | **PipeWire** (the seat's audio server) — mpv's native PipeWire ao. |
| 6 | HW decode | **VA-API with software fallback.** |
| 7 | Subtitles | Embedded + external (.srt/.ass) + full styling **+ online OpenSubtitles fetch**. |
| 8 | Tracks | **Full** audio/subtitle/video-track selection with language labels. |
| 9 | Playlists | **Full** — save/load, queue, shuffle, repeat, gapless. |
| 10 | Local library | Indexed (embedded metadata + local artwork) **+ online TMDB/TVDB scrape**. |
| 11 | Resume | **Per-title resume + continue-watching + watch history.** |
| 12 | Controls | **Full** — speed, A/V-sync offsets, chapters, frame-step, snapshot, A-B loop. |
| 13 | Video adj | **Full** — aspect/zoom/crop/rotate/deinterlace + mpv filters. |
| 14 | Audio proc | **EQ + filters + normalization/ReplayGain + gapless.** |
| 15 | Capture | **Frame snapshot / screenshot** (no stream recording). |
| 16 | Jellyfin conn | **Multi-server + Quick Connect + saved logins.** |
| 17 | JF browse | **Full** — shows→seasons→episodes, movies, music, collections, genres, artwork, Continue-Watching / Next-Up. |
| 18 | Transcode | **Direct-play, transcode (HLS) fallback.** |
| 19 | JF sync | **Full** — report progress, resume across devices, mark-played. |
| 20 | JF content | Movies/shows **+ music + Live-TV/DVR.** |
| 21 | JF search | **Full** cross-library search + filters (genre/year/rating/unwatched) + sort. |
| 22 | JF offline | **Download for offline playback** (managed local cache). |
| 23 | JF users | **User switching** per server. |
| 24 | Streams | Direct URLs (http/hls/rtsp/mms) **+ yt-dlp** for web videos. |
| 25 | **Session roaming** | **Auto-follow** — the session (title/position/queue/tracks) follows the operator's mesh identity; login at another seat offers to resume where it paused, the old seat releases. |
| 26 | **Mesh discovery** | **Auto-discover all mesh media sources** (Jellyfin, DLNA/UPnP, this-player-as-server, mesh file shares) into one Sources list with reachability pips (reuse `mesh_media.rs` / `mdns_relay`). |
| 27 | Mesh library | **Aggregate peers' shared media into one mesh library** alongside local. |
| 28 | Watch-together | **Sync-play across nodes (party mode)** — play/pause/seek propagate. |
| 29 | Cast | **Mesh nodes + DLNA/UPnP + Chromecast.** |
| 30 | Server role | **Yes — share local media to the mesh + a DLNA server** (every node client+server). |
| 31 | UI shape | **Full app** — Sources + Library + Player + Queue + PiP mini-player + OSD. |
| 32 | Immersion | **Immersive fullscreen + auto-hide OSD + PiP mini-player.** |
| 33 | Keys | **Platform-native keymap, rebindable** (consistent with the other egui surfaces). |
| 34 | Look | **Carbon §4 chrome + a translucent dark media OSD** over the video (legibility). |
| 35 | Scope | **Include capture devices** (v4l2/tuner input); DVD/Blu-ray optical **out**. |

## Architecture

- **`crates/desktop/mde-media-egui/`** (new) — the surface + player controller. Wraps
  **libmpv** (system lib + a vendored Rust binding). Chrome/OSD via mde-theme Carbon
  tokens (chrome) + a translucent dark OSD layer (over video). Sources/Library/Player/
  Queue/mini-player views; fullscreen + PiP.
- **Video** scans out on a **dedicated DRM overlay plane** beneath the egui shell —
  coordinate with `mde-shell-egui`'s DRM ownership so egui composites its chrome/OSD
  over the video plane. mpv renders to that plane (VA-API zero-copy where possible).
- **Audio** → PipeWire; the audio-processing chain (EQ/filters/normalization/gapless)
  rides mpv's af graph + PipeWire.
- **Jellyfin client** — a `reqwest` (in-lock) + serde client: Quick Connect + login,
  multi-server, full browse, direct-play/transcode negotiation, progress sync, search/
  filters, offline downloads, user switching, Live-TV.
- **mackesd media service(s)** (mesh-side) — media-source **discovery** (mDNS + mesh
  registry, reuse `mesh_media.rs`/`mdns_relay`), the **media server** (share local
  media + DLNA), **session-roaming** (session record synced to the mesh identity so it
  follows the operator), **sync-play** coordination (party mode), and **casting**
  targets (mesh nodes + DLNA + Chromecast). §6 mesh-side.
- **Capture** — v4l2/tuner input as a source (in scope, Q35).

## Worklist (MEDIA-1..17)

Grouped: engine/AV core (1–6), library/playlists/controls (7–8), the surface (9),
Jellyfin (10–12), streams/capture (13), mesh services (14–17). The `-egui` units
serialize on the surface crate; the mackesd media-service units are file-disjoint from
the surface and from each other where they add distinct workers.

- [ ] **MEDIA-1: libmpv engine + player core.** Add + vendor the libmpv binding (airgap-verify, `--locked`; fall back with an honest note if unfetchable); a `Player` wrapping mpv (load/play/pause/seek/stop, position/duration/state events, track enumeration). Unit-test the state machine against a fake mpv seam; smoke a real short clip on the farm if `libmpv` present, else honest-gate.
- [ ] **MEDIA-2: DRM overlay video plane.** Scan out mpv video on a hardware overlay plane beneath the egui shell, composited with the shell's DRM ownership; egui chrome/OSD draws above it; resize/position track the player pane. Headless/seam-tested; honest-gated where no DRM plane is available.
- [ ] **MEDIA-3: PipeWire audio + processing.** Route mpv audio to PipeWire; EQ + audio filters + loudness normalization/ReplayGain + gapless. Tested via the mpv af-graph config folds.
- [ ] **MEDIA-4: VA-API HW decode + video adjustments.** VA-API decode with software fallback; aspect/zoom/crop/rotate/deinterlace + filters. Tested config folds; honest fallback when no VA-API.
- [ ] **MEDIA-5: subtitles + multi-track.** Embedded + external (.srt/.ass) subtitles with ASS styling/positioning/delay; OpenSubtitles online fetch by hash; full audio/subtitle/video track selection with language labels. Fixture-tested parse + track model.
- [ ] **MEDIA-6: playlists + advanced controls.** Save/load playlists, queue, shuffle, repeat, gapless; speed, A/V-sync offsets, chapters, frame-step, snapshot, A-B loop. Unit-tested queue + control model.
- [ ] **MEDIA-7: local library + resume.** Index chosen folders (+ mesh mounts) from embedded metadata + local artwork, with online TMDB/TVDB scrape; per-title resume + continue-watching + watch history. Tested index/scan + resume store.
- [ ] **MEDIA-8: the mde-media-egui surface shell (Carbon §4).** The full app — Sources + Library browse + Player view + Queue + PiP mini-player + auto-hide OSD; immersive fullscreen; platform-native rebindable keymap; Carbon chrome (no raw hex) + a translucent dark media OSD over video. egui snapshot + headless mount tests.
- [ ] **MEDIA-9: Jellyfin client core.** `reqwest`+serde client: multi-server, Quick Connect + username/password login (saved), full browse (shows→seasons→episodes, movies, music, collections, genres, artwork, Continue-Watching/Next-Up). Fixture-tested API model against recorded responses.
- [ ] **MEDIA-10: Jellyfin playback + sync.** Direct-play/direct-stream vs server-transcode (HLS) negotiation via the libmpv capability set; report progress + resume-across-devices + mark-played; cross-library search + filters + sort; Live-TV/DVR + music. Tested negotiation + progress-report folds.
- [ ] **MEDIA-11: Jellyfin offline + users.** Download titles to a managed local cache for offline playback; multiple user profiles + switching per server. Tested cache lifecycle + user-switch.
- [ ] **MEDIA-12: network streams + yt-dlp.** Open direct stream URLs (http/hls/rtsp/mms) and resolve web-page videos via a bundled yt-dlp. Tested URL detection + the yt-dlp seam (honest-gated when the tool is absent).
- [ ] **MEDIA-13: capture devices.** v4l2 / TV-tuner / capture-card input as a playable source (Q35). Enumerate + open a device; honest-gated when no device present.
- [ ] **MEDIA-14: mackesd mesh media-discovery service.** Auto-discover every mesh node serving media — Jellyfin, DLNA/UPnP, this-player-as-server, mesh file shares — via mDNS + the mesh registry (reuse `mesh_media.rs`/`mdns_relay`); publish `state/media/sources` with reachability. Unit-tested discovery folds; §6 mesh-side.
- [ ] **MEDIA-15: mackesd mesh media server + DLNA.** Share this node's chosen media folders as a mesh media source (so peers' MEDIA-14 finds it) + a DLNA/UPnP server for TVs; aggregate peers' shared media into one mesh library view (MEDIA-8 consumes it). Tested share manifest + aggregation merge; §6.
- [ ] **MEDIA-16: session roaming.** A session record (title/position/queue/tracks/state) bound to the operator's mesh identity + synced (Syncthing/etcd, like bookmarks); on login at a new seat, offer resume-where-paused and release the old seat. Two-seat test: play→switch seat→resume continues. §6 mesh-side.
- [ ] **MEDIA-17: sync-play party mode + casting.** A shared session several seats join where play/pause/seek propagate in sync (watch-together); cast/throw playback to a mesh node, a DLNA/UPnP renderer, or a Chromecast. Tested propagation folds + a gated live cast leg (mirrors mesh_mount gating).

## Acceptance (top-level, §7 runtime-observable)

- Launch `mde-media-egui`: play a local video (DRM overlay plane) + audio (PipeWire),
  with subtitles, track switching, and the full transport/adjust controls; the surface
  is reachable from the dock.
- Connect a Jellyfin server (Quick Connect or login), browse libraries, play a title
  with direct-play or transcode fallback, and resume it on another device.
- The Sources panel lists every auto-discovered mesh media source + Jellyfin server.
- Aggregated mesh library shows peers' shared media; this node also serves media out.
- Pause playback at one seat, log in at another → resume where it paused.
- Start a sync-play session across two seats; play/pause/seek stay in sync.
- Per unit: `build/test/clippy -p <crate>` green, Carbon check (no raw hex on chrome),
  `lint-layered-tiers.sh` clean.

## Out of scope

- Optical media (DVD/Blu-ray menus) — out of v1 scope (Q35); a deliberate scope choice, not a hardware limit (this is a full modern Carbon GUI, not a thin client — capture devices ARE in, per Q35).
- Stream **recording**/broadcasting-out (Q15 = snapshot only).
- Replacing `mde-music`/`mde-musicd` — they coexist (Q3).

## Risks

- **Runtime proof gap (BUG-VIDEO-1):** MEDIA model/surface units can be green while
  the shipped Workstation build still uses `FakeMpv` or paints a placeholder player
  stage. Treat `docs/gpu_encoder.md` and `docs/finalize_validation.md` as the
  required GPU/media validation companion: real libmpv decode must produce visible
  changing frames before this surface is production-complete.
- **libmpv airgap add** (Q1): verify the binding + system lib are buildable early
  (MEDIA-1); fall back to a narrower path with an honest note rather than silently drop
  formats.
- **DRM overlay-plane compositing** (Q4/2): the shell owns DRM; the video plane must
  compose beneath egui without tearing/z-fighting — the hardest integration, prototype
  first; a render-to-egui-texture path is the documented fallback if the plane proves
  unworkable on the fleet GPUs.
- **Egress features** (Q7 OpenSubtitles, Q10 TMDB/TVDB, Q24 yt-dlp, Q28 Chromecast):
  each adds outbound network + an external dependency — gate cleanly, fail soft offline,
  and honor any fleet egress policy.
- **Session-roaming correctness** (Q25): exactly-one-active-seat handoff must not
  double-play or lose position — model it as a single owned lease on the mesh identity.
