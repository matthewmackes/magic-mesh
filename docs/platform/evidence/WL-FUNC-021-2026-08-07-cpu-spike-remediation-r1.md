# WL-FUNC-021 — cross-seat CPU-spike remediation (2026-08-07)

## Finding and correction

The Dell outlier was not a Music playback loop. The live `mackesd` process was
repeatedly doing expensive recovery work while its providers were unavailable:

- `/proc/sys/kernel/random/boot_id` reports `st_size=0` under procfs even when
  its bounded read contains a valid boot UUID. The claimant reader treated that
  pseudo-file metadata as oversized, causing a 10-second peer-record retry and
  repeated certificate-parser work.
- The service aggregator attempted the unavailable resource-publisher secret
  on every eligible catalog fold. A bounded negative retry cache now backs off
  those lookups from 30 seconds to five minutes and resets after success.
- The workstation health sampler launched eight separate `runuser`/PAM
  transitions for each 60-second PipeWire evidence refresh. The five real
  provider checks now run inside one bounded user-session probe, retaining the
  same graph, Pulse, WirePlumber, playback, and capture evidence fields.

The source changes are in:

- `crates/mesh/mackesd/src/telemetry/mod.rs`
- `crates/mesh/mackesd/src/workers/service_aggregator/mod.rs`
- `crates/mesh/mackesd/src/workers/node_grade.rs`

## Farm verification

- `procfs_boot_id_zero_stat_size_is_read_from_content` passed on BigBoy
  `.130`, slot `cpu-overlay-procfs-r1`.
- `publisher_retry_state_is_bounded_and_resets_after_success` passed on farm
  `.50`, slot `cpu-publisher-backoff-r1`.
- `audio_probe_requires_all_bounded_provider_bits` passed on farm `.50`, slot
  `cpu-audio-probe-r1`.
- `rustfmt --check` passed for the complete `node_grade.rs` change on farm
  `.50`. The earlier scoped check of the two other changed files reported only
  pre-existing formatting in the dirty `service_aggregator` file.
- Fedora 44 full RPM cut passed on BigBoy `.130`, slot
  `drain-cpu-fix-audio-r1`, including the shipped `drm,live-vdi,media-mpv`
  shell relink and payload-size gate.

Artifact identity:

```text
magic-mesh-12.1.6-5.x86_64.rpm
sha256 760f2b1e695de17fdddcd71f4d001db7d6d8905f240be9d7293297a7b46283e7
```

## Authorized live deployment

The exact artifact was copied to and force-replaced on:

- seat 15 — `172.20.0.15`
- Dell — `172.20.146.225`

Both seats reported the artifact hash above, `rpm -q magic-mesh` returned
`12.1.6-5.x86_64`, `rpm -V magic-mesh` was clean, `mackesd.service` and the
`mm` user `mde-musicd.service` were active, and the required dynamic libraries
resolved from `ldd`.

## CPU acceptance proof

Command:

```text
MUSIC_CPU_PROOF_HOSTS=172.20.0.15,172.20.146.225
MUSIC_CPU_PROOF_OBSERVE_SECONDS=30
MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS=150
./install-helpers/verify-music-cpu-proof.sh
```

Declared thresholds were maximum `850‰` and mean `500‰` of one CPU, with no
daemon restart during the sample window. Both seats passed:

- seat 15: max `186‰`, mean `106‰`, restarts `0→0`;
- Dell: max `505‰`, mean `429‰`, restarts `0→0`.

This closes the current CPU-spike remediation boundary. It does not close the
full Music/Media epic: both installed seats currently answer ping and
`get-state`, but `list-albums` honestly returns `no Airsonic server configured`;
live provider-loss, renderer, cross-seat handoff, five-seat CPU/NWS, and
hardware playback proof remain separate acceptance gaps.
