# WL-FUNC-021 bounded media recovery evidence — 2026-08-06

## Farm gate

- Farm host: `172.20.0.130` (BigBoy), slot `media-recovery-r2`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-recovery-r2
  ./install-helpers/xcp-build.sh cargo test -p mde-media-core --lib --
  --nocapture`.
- Result: `239 passed, 0 failed`.
- The player now retries an mpv/decoder `EndFile(Error)` up to
  `MAX_RECOVERY_ATTEMPTS` (3), records the current position before retrying,
  resumes from that checkpoint after `FileLoaded`, and emits an explicit retry
  error/state. A fourth consecutive failure reaches `Stopped` with a terminal
  error; the retry does not increment the play count.

## Boundary

This proves bounded fixture/state-machine recovery and position continuity. It
does not claim a live provider reconnect, physical-seat audio, cache-backed
network restoration, or operator-reviewed playback acceptance.
