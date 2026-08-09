# WL-FUNC-021 owner-yield / target-resume routing checkpoint (2026-08-09)

## Behavior delivered

The reachable `mde-musicd` service pump now treats the replicated handoff
completion directory as a shared mailbox. A completion addressed to another
peer is ignored before authorization or cleanup. Only the peer named by
`HandoffCompletion::from_peer` may either resume its exact transferred queue or
retire that completion when its own newest intent no longer authorizes it.

Previously, every daemon evaluated every completion under its local hostname.
The yielding owner (or any unrelated peer) therefore rejected and deleted a
valid target completion before the requested seat could consume it. The new
target-routing decision closes that reachable owner-yield/target-resume race
without weakening stale, superseded, expired, wrong-owner, or replay checks.

## Hostile regression

`shared_handoff_completion_survives_owner_and_bystander_pumps_for_target_resume`
drives one completion through three peer identities:

- the source owner and an unrelated bystander must return `IgnoreForeign`;
- the named target must receive the owner's exact three-song queue, current
  song, and 73,250 ms resume position;
- after a newer target-owned intent supersedes the transfer, that same target
  must return `DropInvalid` rather than resume stale authority.

The production completion pump consumes this routing result before any call to
`clear_completion`, queue persistence, source resolution, or engine start.

## Focused farm proof

BigBoy `172.20.0.130`, explicit slot
`func021-handoff-target-routing-r95`:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=func021-handoff-target-routing-r95 \
install-helpers/xcp-build.sh \
  cargo test -p mde-musicd handoff -- --nocapture

12 passed; 0 failed; 227 filtered out
```

The passing slice includes durable completion admission, alias/oversize/backlog
guards, exact-once source reclaim behavior, exact queue transfer, the new
owner/bystander race regression, and finite handoff seek before audio decode.

`cargo fmt -p mde-musicd -- --check` reported only concurrent formatting drift
in `seat_audio.rs` and `state.rs`; it reported no diff in the changed
`bus_responder.rs`. Those unrelated edits were preserved.

## Remaining live boundary

No package was built or installed and no host was mutated. This checkpoint is
source- and farm-proven. Physical two-seat audible continuity, replicated-file
latency, output-device behavior, and live target acknowledgement remain open.
