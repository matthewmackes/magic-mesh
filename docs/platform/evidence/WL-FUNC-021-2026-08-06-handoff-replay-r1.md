# WL-FUNC-021 — Music handoff alias/replay boundary

Status: durable handoff reader checkpoint complete; live owner-yield/resume
and installed-seat proof remain `Remaining`.

## Change

`mde-musicd` intent and completion readers now require each validated record's
`intent_id` to match the canonical `intent_id.json` filename. Alias files can
therefore not replay an otherwise valid handoff request under a second name.

## Verification

BigBoy `.130`, slot `music-state-handoff-replay-20260806-r1`:

```text
1 passed, 0 failed
```

`git diff --check -- crates/services/mde-musicd/src/state.rs`: pass. Atomic
write behavior remains unchanged; no live runtime handoff was exercised.

## Source hash at capture

```text
f60c5d0595033527026cdd63c9459976d8bd715800738844701306825a7d850e  crates/services/mde-musicd/src/state.rs
```
