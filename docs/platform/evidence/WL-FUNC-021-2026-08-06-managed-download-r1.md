# WL-FUNC-021 — retained-source managed downloads (2026-08-06)

## Implemented slice

`mde-musicd` now resolves typed `download`, `cancel_download`,
`remove_download`, `pin_download`, and `unpin_download` identities through the
same retained catalog/source admission used by typed playback and progress.
Playable non-`legacy` source variants are accepted only when the catalog holds
the exact identity and an admitted provider has the same stable source id;
arbitrary configured providers remain rejected.

Embedded `mde-music-egui` exposes the existing bounded actions in the typed
Library and daemon Album rows, and exposes state-appropriate Pin/Unpin,
Cancel, and Remove controls in Downloads. Buttons require the authenticated
Construct shell writer and never mutate the UI's local snapshot.

## Hostile regression coverage

`admitted_download_selects_retained_nonlegacy_provider` proves that a selected
source-qualified download resolves to the matching second provider despite a
different configured first provider. The existing durable lifecycle test still
covers progress, ready, pin, cancel, remove, and cache cleanup. The Music UI
album regression additionally asserts the exact typed `download` request.

## Farm evidence

- Host `.90`, slot `music-download-daemon-r1`:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-download-daemon-r1 ./install-helpers/xcp-build.sh cargo test -p mde-musicd`
  — **168 passed, 0 failed** across library, binary, and doctest targets.
- Host `.90`, slot `music-download-daemon-fmt-r1`:
  `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-download-daemon-fmt-r1 ./install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check`
  — **passed**.
- Host `.50`, slot `music-download-ui-r2`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-download-ui-r2 ./install-helpers/xcp-build.sh cargo test -p mde-music-egui`
  — **46 passed, 0 failed**.
- Host `.50`, slot `music-download-ui-fmt-r2`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-download-ui-fmt-r2 ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check`
  — **passed**.
- Source SHA-256: `bus_responder.rs` `cf23c0ec2246dda6a6262aabd4848427dbb3f9f1a6c7488613127aee16b99d6a`; `app.rs` `1a6aec06a1be12b3511bed8a5f20a7a16d608a9af0b3995ec02916d464c7edef`.

## Remaining boundary

The provider still needs live credentials/server proof, and real mpv/PipeWire
audio-video, network-loss, cache eviction under live load, package, and seat
acceptance remain open. Farm fixtures prove admission and durable state, not a
successful live download or playback device.
