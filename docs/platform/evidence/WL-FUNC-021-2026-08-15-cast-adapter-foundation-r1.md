# WL-FUNC-021 Cast adapter foundation r1

Date: 2026-08-15

Implemented `crates/services/mde-musicd/src/cast.rs` and added the pinned
`rust_cast` 0.21 dependency. The daemon-side seam validates numeric operator
addresses and bounded names, admits bounded HTTP(S) media commands and finite
seek positions, connects to CASTV2 on port 8009 using the device
protocol's certificate mode, and does not expose sockets or third-party types
to the UI.

Farm validation:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-cast-farm6 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd cast -- --nocapture
```

Result: 10 tests passed, 0 failed. The adapter now dispatches load/play/pause/
seek through the default media receiver. This farm evidence does not claim a
live media URL was loaded or that renderer ownership was committed; those are
post-release validation and projection work respectively.

Opt-in live adapter validation, run with `MDE_CAST_LIVE_TARGET=172.20.146.150`,
passed 7/7 on the farm. The Rust `rust_cast` connection itself successfully
established a CASTV2 session with the operator-supplied receiver. This still
does not claim media URL delivery or ownership commit.

Opt-in live media validation additionally ran with
`MDE_CAST_LIVE_MEDIA_URL=https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerEscapes.mp4`.
The adapter loaded that public MP4 through the default media receiver and then
paused it successfully: 8/8 Cast-filtered tests passed. This closes the live
Cast media load/control proof; receiver discovery projection and renderer
ownership commit remain separate boundaries.

The playback projection now admits an operator-configured numeric Cast target
(`MDE_MUSIC_CAST_ADDRESS` plus optional `MDE_MUSIC_CAST_NAME`) as an explicit
`cast_renderer` row. The DIAL parser also validates bounded friendly name,
UDN, model, and numeric address identity. It remains unavailable until the blocking provider lane
performs live verification, so projection cannot fabricate renderer ownership.
Generation-bound ownership records use atomic same-directory replacement and
are covered by the focused farm test. The provider-lane helper now enforces
command-success-before-commit; mesh handoff schema integration remains.
