# WL-FUNC-021 bounded provider-loss reconnect implementation — 2026-08-06

Status: implementation and farm-test evidence; live provider-loss transition
remains unproven because the approved seat was observed read-only and no outage
was naturally present.

## Implementation

`crates/services/mde-musicd/src/engine.rs` now distinguishes a mid-stream
provider error from a clean EOF and, when the source is a Subsonic `/stream`
URL with a valid song id, retries the same logical track from the audible
playhead. The reconnect path:

- waits on the existing bounded exponential schedule (1, 2, and 4 seconds);
- allows at most `MAX_MIDTRACK_RECONNECTS = 3` attempts;
- uses the provider's integer-second `timeOffset` resume contract;
- clears decoded-but-not-yet-audible samples before retrying, so buffered audio
  is not replayed;
- never writes an offset response into the complete-track cache; and
- refuses arbitrary direct/radio URLs that cannot prove a resumable endpoint,
  then advances without a from-zero fallback.

## Farm verification

BigBoy `172.20.0.130`, isolated slot
`MCNF_BUILD_SLOT=music-reconnect-full-20260806-r1`:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=music-reconnect-full-20260806-r1 \
./install-helpers/xcp-build.sh cargo test --locked -p mde-musicd --lib -- --nocapture
result: 176 passed, 0 failed
```

The focused engine lane in slot `music-reconnect-engine-20260806-r1` also
passed 21/21, including the resumable URL identity/offset and reconnect-budget
tests. Local `git diff --check` passed.

## Live observation

The observation-only helper
`install-helpers/verify-music-live-provider-loss.sh` was run against seat 15
with a six-second bounded window:

```text
[sample 1] service=active provider=ok catalog=ok state=ok class=healthy
[sample 2] service=active provider=ok catalog=ok state=ok class=healthy
[REFUSAL] no natural provider loss was observed before the deadline
[REFUSAL] live provider-loss transition was not fully observed
```

The helper does not interrupt playback, alter network state, restart a service,
or expose credentials. Live provider loss/recovery, audible continuity, and
current-package seat proof therefore remain open rather than being inferred
from the implementation or fixture tests.
