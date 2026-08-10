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

