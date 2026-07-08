# GPU-Accelerated Media Pipeline Plan

Status: planning/validation draft, 2026-07-06.

This document covers the GPU side of the native media player work: real video
decode/display, thumbnail and metadata acceleration, and the validation gates that
prove the shipped shell is not using `FakeMpv` or a placeholder video rectangle.

## Objective

Make `mde-media-egui` a real VLC-class player in the Quasar shell:

- local AV1/Opus/Matroska and common H.264/H.265 media plays with video and audio;
- hardware decode is used where the seat exposes it, with an honest software
  fallback;
- the UI renders actual frames, not a simulated playback state;
- library indexing can generate thumbnails, probes, and media metadata without
  blocking the shell;
- validation can run in three stages: fixture, farm, and live seat.

## Current Gap

`docs/WORKLIST.md` marks MEDIA-1..18 complete, but `BUG-VIDEO-1` records two
remaining runtime blockers:

- the shipped player may still build without the real `mpv` feature, leaving the
  surface backed by `FakeMpv`;
- the player stage can paint a placeholder instead of displaying frames from
  mpv's render path or a DRM overlay plane.

The validation plan below treats those as release blockers for the media player.

## Recommended Path

Use a two-step display path:

1. **Render API to egui texture first.**
   This is the fastest way to prove real decode and visible frames inside the
   existing shell. It should use libmpv's render API to produce frames that the
   shell uploads to an `egui::TextureHandle`. This closes the FakeMpv/placeholder
   gap and gives stable headless-ish fixture tests around frame flow.

2. **DRM overlay plane second.**
   Keep the locked MEDIA-2 target: a hardware overlay plane under egui chrome/OSD
   for lower power and smoother playback. Treat it as a performance path that must
   pass live-seat validation, not as the first proof that playback works.

This preserves the existing design lock while making the completion path testable.

## Architecture

### Crates

- `mde-media-core`
  - owns player state, mpv command/property folds, stream classification, playlists,
    subtitles, audio/video configuration, and metadata models;
  - real engine is compiled behind the `mpv` feature;
  - validation must prove the release build enables the real engine.

- `mde-media-egui`
  - owns Sources/Library/Player/Queue/PiP/OSD UI;
  - must expose a frame sink abstraction that can accept either an egui texture
    frame or a DRM overlay plane handle;
  - must not silently fall back to `FakeMpv` in a production Workstation build.

- `mde-shell-egui`
  - mounts the media surface;
  - owns DRM and must broker any overlay-plane allocation;
  - release features should include the real media path, e.g. a shell feature that
    forwards to `mde-media-egui/mpv`.

### Frame Flow

Minimum viable real playback:

1. UI selects a local or remote media item.
2. `mde-media-egui` issues `Player::load`.
3. real `MpvEngine` opens the URL/path and emits state plus decoded frame events.
4. the frame sink uploads the newest frame to an egui texture.
5. `player_stage` paints the texture with Carbon OSD above it.
6. audio goes through mpv/PipeWire.

Overlay playback:

1. shell allocates or selects a DRM overlay plane compatible with the active CRTC;
2. mpv/VA-API path produces an importable buffer or an agreed scanout target;
3. shell positions the overlay plane under egui chrome and OSD;
4. fallback to texture mode if plane allocation, format import, or synchronization
   fails.

### Metadata/Thumbnail Pipeline

The GPU-accelerated metadata pipeline is separate from interactive playback. It
should serve library indexing and Files previews:

- probe container metadata with ffprobe/mpv/metadata readers;
- generate bounded thumbnails and preview strips;
- compute media fingerprints or stable IDs for resume/library dedup;
- write results to a cache keyed by file identity and mtime;
- throttle work so indexing cannot starve playback or shell rendering.

Use hardware decode for thumbnail extraction when available, but keep software
fallback. Cache outputs as ordinary files plus a small index, not GPU-only state.

## Build Requirements

- farm build VMs need the libmpv development package for link-time validation;
- Workstation RPM needs runtime libmpv and codecs sufficient for AV1/H.264/H.265,
  Opus/AAC/FLAC, Matroska/MP4/HLS;
- release build path must enable the media real-engine feature;
- `rpm -qlp` should prove the media surface and relevant runtime dependencies ship.

## Validation Gates

### L0: Pure Model

- playlist, subtitle, audio, video, stream, Jellyfin, and metadata folds pass unit
  tests without libmpv;
- production-feature detection has a test that fails if the release shell is built
  without the real media engine.

### L1: Fixture Decode

- short fixture files decode through real mpv when the feature and system lib are
  present;
- the test captures at least one nonblank frame checksum;
- audio state reaches playing or a typed no-audio-device gate;
- no `FakeMpv` type is used in the release feature configuration.

### L2: Farm Build

- `xcp-build.sh rpm` completes with media real-engine features enabled;
- the generated RPM contains the media player and shell;
- `rpm -qlp` evidence is captured in the worklist.

### L3: Live Seat

On Eagle or the clean `.2` seat:

- open `~/Downloads/*.mkv` AV1/Opus/Matroska;
- video frames visibly change over time;
- audio plays through PipeWire;
- pause/seek/fullscreen/OSD controls work;
- logs show real mpv engine, selected hwdec mode, and whether VA-API was active;
- fallback mode is explicit if overlay plane is unavailable.

## Open Decisions

- First live implementation target: egui texture first, then DRM overlay plane
  optimization. Recommended: yes.
- Exact feature names for release builds. Recommended: `media-mpv` on
  `mde-shell-egui`, forwarding to `mde-media-egui/mpv`.
- Whether thumbnail generation belongs in `mde-media-core`, `mde-files-egui`, or a
  small shared media-index crate. Recommended: core model in `mde-media-core`,
  callers in Files/Media.
- Whether GPU thumbnail work should be a daemon queue or local UI task.
  Recommended: daemon or background worker for library indexing; local task for
  one-off previews.

## Next Work Items

1. Add the release feature wiring for real mpv.
2. Install/build-gate libmpv development packages on the farm.
3. Replace placeholder `player_stage` with a real frame sink.
4. Add fixture decode tests that assert nonblank changing frames.
5. Add live-seat validation steps for AV1/Opus/Matroska and overlay/texture mode.
