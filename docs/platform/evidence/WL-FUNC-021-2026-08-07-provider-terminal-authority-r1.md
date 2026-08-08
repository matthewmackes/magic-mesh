# WL-FUNC-021 provider-terminal playback authority (2026-08-07)

The Music daemon now clears its authoritative playback state when a decode
thread exhausts its admitted provider/reconnect path or reaches normal end.
The responder also publishes `playing=false` for an ended inactive engine, so
a failed provider cannot leave a silent seat claiming mesh playback ownership
until the stale-peer timeout.

The focused BigBoy regression gate passed:

```text
1 passed; 0 failed; 190 filtered out
```

The complete current daemon gate then passed on BigBoy `.130` in slot
`musicd-terminal-authority-r1`:

```text
191 unit tests passed; 0 failed
0 main tests; 0 failed
0 doctests; 0 failed
```

The regression serves an HTTP 503 provider response through the real engine
handle and verifies that decode completion clears both `playing` and active
authority. This is bounded source/fixture evidence; a natural live provider
loss, audible continuity, and cross-seat owner-yield/resume remain open.
