# WL-FUNC-021 — physical two-seat handoff preflight r5

Date: 2026-08-09  
Result: **REFUSED before mutation**

## Scope

This run evaluated Basement seat 15 and Eagle for one bounded, typed Music
owner-yield/target-start handoff. Eagle was used only for the operator-approved
conditional preflight. No playback action, takeover intent, package change,
service restart, or seat alert was issued because the mandatory package/revision
and peer-admission gates did not pass.

## Bound pre-state

Both seats were reached as `mm` through the governed key. The checks bound the
installed RPM identity, RPM-owned daemon payload digest, service state, restart
count, queue shape, peer snapshots, and handoff-record counts.

| Property | Basement seat 15 | Eagle |
|---|---|---|
| Hostname | `Basement-Test-Workstation` | `T470S-EAGLE` |
| Installed RPM | `magic-mesh-12.1.6-23.x86_64` | `magic-mesh-12.1.6-12.x86_64` |
| `/usr/bin/mde-musicd` SHA-256 | `7015490f5447386d2a23eb0e81bdae1b219bead87de7293b906aed48b4972e6c` | `3125900c3405fac5b9ec0143aa873e682fa2219e11ed9bd00cd278181517e5a0` |
| `mde-musicd.service` | active, `NRestarts=0` | active, `NRestarts=0` |
| Playback state | idle | idle |
| Queue | one entry, cursor 0 | empty |
| Current identity | redacted legacy direct-stream identifier; SHA-256 `350af7fa56d1b8b48877b60164008ff22de249c674f9ef04ce5448f9ae038fc9` | none |
| Typed preferred source | absent | absent |
| Handoff intents / completions | 0 / 0 | 0 / 0 |

The pre-existing direct-stream identifier was observed only to classify and
hash the queue boundary. It was not printed into this evidence, selected,
replayed, copied to Eagle, or used for playback.

## Admission decision

The checked-out release authority is RPM release `23`. Seat 15 matches it;
Eagle is eleven package iterations behind and executes a different daemon
payload. Exact release and runtime-revision binding therefore fails.

Peer admission also fails independently:

- Seat 15 has no `T470S-EAGLE` peer snapshot.
- Eagle's seat-15 snapshot was approximately 28.5 million ms old at collection,
  far beyond the production `STATE_STALE_MS=15_000` bound.
- Eagle has no local queue, while seat 15 has no typed admitted source binding.

These facts do not permit a target to be inferred from network reachability.
They also cannot prove exact queue transfer or a finite target playhead. The
run therefore stopped before creating a one-use intent, starting audio, or
asking either owner to yield.

## Farm regression

Machine 193 (`172.20.0.90`), explicit slot `func021-handoff-r5`, ran the focused
production handoff regression and state-contract tests. The slot was removed
after completion.

- `two_seat_handoff_is_exact_once_replay_safe_and_failure_honest`: 1 passed,
  0 failed.
- `state::tests`: 18 passed, 0 failed.
- The explicit 2.5 GiB slot was removed; its disposable build artifacts are not
  recoverable.

The focused regression covers exact queue/cursor transfer, finite playhead,
single owner, one-use replay refusal, target-start failure, and source recovery.
It is code-level evidence only and does not substitute for physical-seat proof.

## Cleanup and remaining proof

The live seats remained in their original stopped/neutral state: both daemons
idle, zero handoff intents, zero completions, and zero service restarts. No
speaker, PipeWire capture, rendered frame, or human audio judgment was attempted
or claimed.

Physical acceptance remains open. A later run must first place both governed
seats on the same bound package/runtime revision, establish reciprocal fresh
peer admission, and select a non-sensitive typed catalog fixture. Only then may
it issue one bounded signed takeover and prove exact queue identity, finite
playhead continuity, sole ownership, replay refusal, and cleanup/recovery.
