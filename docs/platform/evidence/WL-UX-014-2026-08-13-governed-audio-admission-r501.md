# WL-UX-014 — governed A–F audio admission (r501)

- Scope: advance S2 without claiming unavailable authored media. Kiron asset
  manifest schema v2 now requires one distinct audio cue for every A–F grade in
  addition to the complete live-3D/pre-rendered/static scene ladder.
- Package boundary: every scene and cue carries approved SPDX licensing plus
  creator, original/third-party origin, and a content-addressed source revision.
  Audio bytes are independently hashed and size bounded; admitted WAV files
  must be immutable regular files with matching channel, sample-rate, and frame
  metadata, uncompressed 16/24-bit PCM, and a maximum 15-second duration.
- Hostile coverage: the executable self-test rejects scene digest tampering,
  incomplete static fallback, multiply-linked scene assets, incomplete A–F
  audio coverage, and false waveform metadata.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=ux014audio install-helpers/xcp-build.sh sync`, then in the isolated farm workspace
  `python3 -m py_compile install-helpers/verify-kiron-assets.py && python3 install-helpers/verify-kiron-assets.py --self-test`.
- Result: PASS — `Kiron asset manifest verification self-tests passed`.
- Honest remaining S2 acceptance: author and license the actual six original
  A–F source scenes, recovery-transition media, three visual fallback tiers,
  and six final cues; produce their schema-v2 manifest; then wire that admitted
  package into the release payload. This change deliberately contains no
  placeholder production assets and is not live/render proof.
