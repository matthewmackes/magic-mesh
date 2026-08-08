# WL-FUNC-021 — daemon-owned renderer revocation, recovery, and continuation (2026-08-08)

## Requirement advanced

FUNC-021 S2/S3 requires daemon-owned real playback with honest output state and
recovery. Before this slice, `mde-musicd` retried when no default output device
existed at startup, but a cpal stream failure after successful acquisition only
logged a warning. The daemon retained that dead engine, continued projecting
`audio_available`, and stale MPRIS/transport handles could claim playback until
the service was manually restarted.

The native engine now treats cpal's asynchronous stream-error callback as loss
of physical renderer authority. It atomically marks the engine failed, yields
playing/seek authority, stops decoding, discards samples that can no longer be
proven audible, and refuses playback or resume through stale handles. The
daemon detects that failed engine, tears down its matching MPRIS surface, and
uses the existing bounded ten-second acquisition cadence to construct a fresh
engine against the current default output device. While no replacement exists,
the engine is absent and Music reports audio unavailable rather than inventing
success.

The same daemon-owned recovery path now retains a bounded interruption intent
for finite media that was actively playing. The cpal failure callback captures
the last audible position before revoking the old engine. After the daemon
acquires a fresh physical renderer, it resolves the interrupted track through
the normal admitted-source/cache policy and resumes at that position. The
intent includes a monotonic process generation, the exact queue identity, and
the last-seen mutating control cursors. Any queue change or control observed
while output is absent—including a refused Stop—invalidates the intent before
the replacement engine can emit audio. Idle, paused, and unseekable/live media
are not automatically restarted.

## Hostile regression

The regression seeds an apparently active renderer with decoded buffered audio,
injects renderer failure through the same production revocation helper, and
proves that:

- playback and seek authority are revoked;
- unaudible buffered samples are discarded and accounting is rewound to the
  last emitted frame;
- resume cannot reassert playing state; and
- a stale engine handle refuses a new playback start.

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=func021-renderer-recovery-r1 \
install-helpers/xcp-build.sh cargo test -p mde-musicd \
  renderer_failure_revokes_authority_and_refuses_stale_restart \
  --locked -- --nocapture
```

Result: `1 passed; 0 failed; 206 filtered out`; the mde-musicd library and main
test binaries compiled successfully.

The continuation regression creates an interrupted finite-track generation,
injects a hostile Stop cursor while the renderer is absent, and proves that the
old generation cannot resume or consume a newer recovery record. It also proves
that replacing the queued track prevents continuation even when the engine is
otherwise idle.

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=func021-renderer-auto-resume-r1 \
install-helpers/xcp-build.sh cargo test -p mde-musicd \
  renderer_recovery_refuses_stale_generation_after_intervening_control \
  -- --nocapture
```

Result: `1 passed; 0 failed; 207 filtered out`; both mde-musicd test binaries
compiled successfully.

The package-wide format check on `.90`, slot
`func021-renderer-fmt-r1`, remained red because it found pre-existing formatting
drift across unrelated Music files. No broad reformat was applied over
concurrent work. `git diff --check` passes for the working tree.

## Remaining boundary

This is source and farm proof of fail-closed renderer restart/reacquisition and
generation-safe interrupted-track continuation. It does not prove a physical
PipeWire restart, audible speaker output, owner-yield and target-resume across
two seats, or DLNA/Chromecast hardware. Those live acceptance boundaries remain
open under WL-FUNC-021.

## Landing source identity

- `crates/services/mde-musicd/src/engine.rs`:
  `f381312f81015a6d69aea1578e39f1c82e6959370a880a63f40ff25ea656d8ed`
- `crates/services/mde-musicd/src/bus_responder.rs`:
  `53bf7cad2515e06ecd30ea03cc9b5b5780abdffe2c5020e6e84aaa5f16dc9637`
