# WL-FUNC-021 — workerless embedded Music authority (2026-08-06)

## Implemented slice

`mde-music-egui` now has an explicit `new_embedded_with_ctx` constructor that
does not start the standalone Airsonic worker. The standalone `new_with_ctx`
path retains its bounded worker fallback for the independent Music binary.
`mde-shell-egui` uses the embedded constructor, so the shell's daemon-retained
workspace reader and authenticated Bus writers are the only active embedded
Music provider/action seams.

Credential retry is also disabled for workerless instances. This prevents a
late credential file from silently starting a competing provider, store, or
playback authority after the shell has mounted the daemon-owned surface.

## Hostile regression coverage

`embedded_constructor_does_not_start_standalone_worker` constructs the shell
mode with a clean egui context and asserts both that no worker command channel
exists and that the worker mode is disabled.

## Farm evidence

- Host `.50`, slot `music-embedded-workerless-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-embedded-workerless-r1 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — **47 passed, 0 failed** across library, binary, and doctest targets.
- Host `.50`, slot `music-embedded-workerless-fmt-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-embedded-workerless-fmt-r1 ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check`
  — **passed**.
- Host `.90`, slot `music-embedded-shell-r1`:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-embedded-shell-r1 ./install-helpers/xcp-build.sh cargo test -p mde-shell-egui music`
  — shell test binary compiled; the filter matched no shell test names (**0 executed, 1,451 filtered**).
- A package-wide shell format check remains mixed by unrelated pre-existing
  drift; no bulk formatter rewrite was applied.
- Source SHA-256: `app.rs` `6fac47bc9183645b7bee6a4990c5876e0c96c546d5fb543e9baaf6a480b63ca3`; `main.rs` `b66f61404d99bb5f18acf61f4e36a77e8fdea0a669c00bca186b1b9a74aac34a`.

## Remaining boundary

This removes the embedded worker authority, but live provider/audio/video,
mpv/PipeWire/DRM, cast/handoff, cache/network-loss, package, and seat proof
remain open. The standalone binary's fallback is retained intentionally until
its independent migration is complete.
