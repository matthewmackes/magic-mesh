# Media/GPU Validation Runbook

Status: draft, 2026-07-06.

Use this after the media GPU pipeline is wired. It is deliberately staged so we
can promote from fixture to farm to live seat without claiming production media
support from a narrow test.

## Preconditions

- Release build enables the real media engine, not `FakeMpv`.
- Farm image has the libmpv development package available.
- Workstation RPM has runtime libmpv and codec dependencies.
- A short test media set exists:
  - AV1 + Opus in Matroska;
  - H.264 + AAC in MP4;
  - H.265 + AAC or Opus where hardware permits;
  - subtitle sidecar `.srt`;
  - a short HLS URL fixture or local mock.

## Stage 1: Static/Build Proof

Commands:

```bash
./install-helpers/xcp-build.sh cargo test -p mde-media-core
./install-helpers/xcp-build.sh cargo test -p mde-media-egui
MCNF_BUILD_SHAPE=big ./install-helpers/xcp-build.sh rpm
rpm -qlp ~/mcnf-release-artifacts/magic-mesh-*.rpm | rg 'mde-shell-egui|mde-media|mde-shell-egui.service'
```

Evidence required:

- tests pass;
- release build uses the real media feature;
- RPM contains the shell and media assets;
- no packaging path points at a missing binary.

## Stage 2: Fixture Runtime Proof

Run on a farm or dev host with libmpv:

- load each fixture through the real `MpvEngine`;
- capture frame checksums before and after playback starts;
- assert checksums are nonblank and change over time;
- assert play/pause/seek changes the reported player state;
- assert subtitle and track-selection commands reach mpv;
- record whether hwdec is `auto-safe`, `vaapi`, or software.

Pass condition:

- at least one frame is decoded and displayed through the frame sink for every
  fixture format, or the test reports a typed environment gate.

## Stage 3: Live Seat Proof

Target: Eagle `.13` or clean `.2` Workstation seat.

Steps:

1. Install the candidate RPM.
2. Reboot if validating boot-to-seat and media together.
3. Open the Media surface from the shell.
4. Play the AV1/Opus/Matroska sample from `~/Downloads`.
5. Verify:
   - visible video changes;
   - audio plays through PipeWire;
   - OSD overlays above video;
   - seek/pause/fullscreen work;
   - engine log names real mpv, not FakeMpv;
   - hwdec/overlay mode is logged honestly.

Pass condition:

- the operator can watch the local video with audio and controls from the Quazar
  shell without launching an external player.

## Stage 4: Promotion Evidence

Record in `docs/WORKLIST.md`:

- RPM filename and build host;
- `rpm -qlp` lines proving media and shell assets;
- farm test commands and counts;
- live seat, date, sample format, engine mode, and observed result;
- any honest fallback, e.g. "texture mode active; overlay plane unavailable".

Do not close the media/video bug if only model tests pass. Completion requires a
real frame path and live-seat proof.
