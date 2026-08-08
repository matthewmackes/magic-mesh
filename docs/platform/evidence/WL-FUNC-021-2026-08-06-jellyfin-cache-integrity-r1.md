# WL-FUNC-021 Jellyfin offline-cache integrity evidence — 2026-08-06

## Scope

Jellyfin offline media writes now use same-directory temporary files, synced
contents, atomic rename, and parent-directory sync for both media bytes and the
manifest. Cache availability and local playback paths reject unsafe manifest
filenames, missing files, symlinks, and files whose size no longer matches the
retained manifest. Media UI offline rows use the same verified-availability
predicate, so a stale manifest entry is not presented as playable.

## Focused evidence

- `crates/desktop/mde-jellyfin/src/cache.rs` adds atomic cache/manifest writes,
  safe single-component cache names, and hostile missing/truncated/path tests.
- `crates/desktop/mde-media-egui/src/model.rs` filters offline rows through the
  verified cache availability path.
- Farm Jellyfin test:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=jellyfin-cache-integrity-r1 ./install-helpers/xcp-build.sh cargo test -p mde-jellyfin`
  — 88 unit + 12 browse + 9 playback + 1 doctest passed; 0 failed.
- Farm Jellyfin format:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=jellyfin-cache-integrity-fmt-r2 ./install-helpers/xcp-build.sh cargo fmt -p mde-jellyfin -- --check`
  — passed.
- Farm Media UI test:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-cache-integrity-r1 ./install-helpers/xcp-build.sh cargo test -p mde-media-egui`
  — 104 passed, 0 failed; doctests: 0 passed, 0 failed.
- Farm Media UI format:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-cache-integrity-fmt-r1 ./install-helpers/xcp-build.sh cargo fmt -p mde-media-egui -- --check`
  — passed.
- Local `git diff --check` — passed.

## Source hashes

- `crates/desktop/mde-jellyfin/src/cache.rs`
  — `3f84e0c4a629d7acb017eac3b77e8c1ed2a24a21e81845972656942581f70fe1`
- `crates/desktop/mde-media-egui/src/model.rs`
  — `b354a6fb650fdc4572284584aea50d4dae134eebeff7b13da229fc693aacc977`
- `crates/desktop/mde-media-egui/src/app.rs`
  — `bf29d8a2cfa79fcd2e068fe8e4168a7e268a426284e130c7b8bb1373cd421a46`

## Limitations / remaining acceptance

This proves local cache integrity and fixture-backed offline playback only. It
does not prove live Jellyfin download/network-loss recovery, mpv/PipeWire/DRM
audio-video output, casting, package/RPM promotion, or Dell/seat-15 live
acceptance.
