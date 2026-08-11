# WL-FUNC-011 — conflicting collaboration event duplicates (r157)

Date: 2026-08-10

The collaboration merge path now rejects signed reuse of an event ID when the
canonical contents differ, both against the durable log and within one batch,
while retaining exact duplicate idempotency. Farm proof on `.90`:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=func011-collab-dup-r157d
install-helpers/xcp-build.sh cargo test -p mde-collab-core --lib \
  merge_rejects_conflicting_event_id_reuse_in_log_and_batch -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 98 filtered out
```

Follow-up on 2026-08-11 closed the same invariant at the durable actor-log
boundary used directly by local collaboration actions and imports. Exact signed
replay remains idempotent, while a reused event ID with different contents now
returns typed `ConflictingEventId`, including after reopening the JSONL log.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
install-helpers/xcp-build.sh cargo test -p mde-collab-core \
  log::tests::file_actor_log_refuses_conflicting_event_id_across_restart \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 100 filtered out
```
