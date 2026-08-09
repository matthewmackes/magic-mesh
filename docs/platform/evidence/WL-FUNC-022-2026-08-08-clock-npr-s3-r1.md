# WL-FUNC-022 governed NPR Clock sources S3 — 2026-08-08

`mde-musicd` now retains NPR News Now as the stable catalog identity `500005`
and resolves that identity to the newest admitted hourly episode at ring time.
Clock never receives the provider URL or resolved provider identity. A
configured NPR member station remains a separate stable Radio catalog item,
also resolved only inside Music.

The retained News Now resolution is bounded and fail closed. Missing, deleted,
malformed, future-dated, older-than-two-hours, unauthorized, or unreachable
catalog state enters the existing bundled-tone fallback policy. Refreshing an
empty News Now response removes the retained alias rather than replaying an old
episode. Alert playback remains isolated from Music queue, ownership, history,
and bookmarks.

## Verification

Machine 196 (`172.20.0.196`), slot
`func022-clock-npr-sources-s3-r1`:

```text
cargo test --locked -p mde-musicd --lib clock_ -- --nocapture
11 passed; 0 failed; 210 filtered out

cargo test --locked -p mde-musicd --lib news_now_ -- --nocapture
3 passed; 0 failed; 217 filtered out
```

The groups contain 12 unique tests. Named coverage proves newest-episode
refresh, stable `500005` resolution, a separate live-station identity,
malformed/stale/unauthorized/unreachable/deleted refusal, immediate fallback,
and queue-independent Clock audio policy. Scoped formatting and diff checks
also passed. Tests made no live network calls.

## Remaining acceptance gap

Live provisioning of the official NPR feed and a configured member station,
governed local-file audio, typed resolve/preview operations, persisted Music
audio replay, concurrent-occurrence arbitration, seat-wide WirePlumber ducking,
and physical audible-output/source-loss proof remain. FUNC-022 stays
`Remaining`.
