# WL-FUNC-021 — idle playback-state coalescing (r155)

Date: 2026-08-10

`mde-musicd` now suppresses unchanged idle playback-state writes after the
transition publication while retaining transition changes and periodic
playback heartbeats. This reduces needless mesh churn during seat idle time.

## Farm proof

BigBoy (`172.20.0.130`), slot `func021-idle-coalesce-r155`:

```text
cargo test -p mde-musicd --lib bus_responder::tests::unchanged_idle_state_is_coalesced_after_transition_write -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 240 filtered out
```

Live seat-15 steady-state CPU retest remains open.
