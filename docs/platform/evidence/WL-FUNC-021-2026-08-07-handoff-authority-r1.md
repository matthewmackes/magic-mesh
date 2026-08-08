# WL-FUNC-021 typed Music handoff completion authority (2026-08-07)

The current daemon handoff path now treats completion as an explicit terminal
state. A successful transfer completion is accepted only for the matching
request/target, late or duplicate completion cannot restore stale ownership,
and a failed completion leaves the local playback authority honest. This keeps
the embedded UI's typed target view aligned with the daemon's ownership state.

Farm verification on BigBoy:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-handoff-authority-r2 \
  ./install-helpers/xcp-build.sh cargo test -p mde-musicd handoff_completion --locked -- --nocapture
PASS: 3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=musicd-handoff-full-r2 \
  ./install-helpers/xcp-build.sh cargo test -p mde-musicd --locked -- --nocapture
PASS: 192 passed, 0 failed
```

This is contract/fixture evidence. Live owner-yield/resume across two seats is
still open and is not claimed by this checkpoint.
