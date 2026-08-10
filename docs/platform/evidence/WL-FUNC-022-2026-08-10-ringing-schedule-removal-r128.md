# Ringing Clock schedule removal checkpoint

Date: 2026-08-10  
Epic: `WL-FUNC-022` S2/S3

## Defect and correction

Removing a Clock schedule deleted its scheduler row without terminating a
currently ringing occurrence. The occurrence then had no schedule through which
it could be acknowledged or auto-silenced, and no durable Music Stop transition
was produced. Alert audio could consequently outlive both schedule removal and
daemon restart.

Schedule removal now performs one staged Clock-state mutation that:

- converts every exact ringing occurrence to `Stopped` through the ordinary
  acknowledgement convergence path;
- derives a deterministic request/occurrence-bound acknowledgement ID;
- persists the terminal occurrence and corresponding durable Music Stop outbox;
- removes pending scheduled/snooze child occurrences; and
- deletes the schedule only after those transitions succeed.

Unknown schedule removal remains idempotent. Peer-origin removal remains
non-authoritative and refused.

## Focused farm proof

Host `.90`, slot `func022-clock-remove-ringing-r1`:

```text
cargo test -p mackesd \
  removing_a_ringing_schedule_atomically_stops_audio_and_persists_the_terminal_occurrence \
  -- --nocapture

1 passed; 0 failed; 4673 filtered out
```

The test advances a real fixture occurrence to Ringing, removes its schedule,
reloads the durable snapshot, and verifies the exact single Stop outbox record.
`git diff --check` passed. Physical audible output and restart/reboot acceptance
remain live boundaries.
