# WL-FUNC-022 queue-independent Clock audio S3 — 2026-08-08

mde-musicd now owns a bounded Clock-alert audio authority keyed by exact
occurrence, global event, and generation. Requests pass the existing signed
Music action authorizer. Callers may name only a licensed bundled tone or a
stable retained Music catalog identity; raw URLs, paths, and commands are not
part of the contract.

Clock alerts use a renderer separate from the Music queue. Starting an alert
does not modify Music queue content, cursor, ownership, history, bookmarks, or
MPRIS state. Active Music gain is ducked to 25 percent of its exact prior value
and restored on Stop, Snooze, replacement, provider loss, renderer loss, and
daemon-loop exit. Governed Music resolution failures start the requested
bundled fallback when available and publish the provider state.

Request replay is effect-idempotent in process. Stale generations and conflicting
request IDs are refused. An asynchronous renderer failure clears the active
occurrence, revokes buffered output, publishes `ProviderUnavailable`, restores
gain, and prevents the same generation from auto-restarting.

The production serve loop now observes the first audible frame against an
injected 3,000 ms deadline. A governed source that fails or remains inaudible at
the deadline transitions to the bundled fallback immediately; the transitioned
status is retained for exact replay without repeating effects.

The Clock worker now commits an exact occurrence/global-event/generation audio
request to its sole-writer SQLite outbox in the same transaction as the Clock
snapshot, then signs and publishes it. Unacknowledged Start, Stop, and Snooze
effects replay after worker restart under the same stable request ID. Only the
authorization, issuance time, and expiry are refreshed; mde-musicd compares the
entire remaining body and refuses a changed effect under that ID.

## Verification

- `.50`, slot `func022-clock-audio-s3-r1`: `mde-musicd` Clock authority tests
  passed 4/4, with 209 unrelated tests filtered.
- The shared Clock audio contract tests passed 6/6, with 475 unrelated tests
  filtered.
- `.50`, slot `func022-clock-audio-emission-s3-r2`: mackesd Clock worker tests
  passed 2/2 and mde-musicd Clock authority tests passed 4/4. The worker proof
  covers durable restart replay, receipt acknowledgement, and exact
  Ringing-to-Snoozed emission using the prior ringing generation.
- `.196`, slot `func022-clock-audio-full-r3`: the complete mde-musicd library
  suite passed 213/213 with no ignored tests.
- `.196`, slot `func022-clock-audio-policy-s3-r2`: the focused audibility,
  fallback, exact duck/restore, provider-loss, and replay suite passed 7/7, with
  209 unrelated tests filtered.
- Scoped formatting and `git diff --check` passed.

## Remaining acceptance gap

Audio replay state across mde-musicd restart, NPR newest-hourly and configured
live-station resolution, seat-wide WirePlumber ducking, concurrent occurrences,
and live PipeWire/hardware output proof remain. FUNC-022 stays `Remaining`.
