# WL-FUNC-011 alert delivery restart boundary — r18

Date: 2026-08-09

## Boundary

WL-FUNC-011 S3 requires canonical alerts to replay and deduplicate
deterministically offline. The alert relay previously acknowledged an event in
memory before either delivery path succeeded and discarded that acknowledgement
on daemon restart. A failed Bus and notification fallback was therefore
suppressed until restart, while every successfully delivered retained event was
replayed after restart.

The worker now:

- acknowledges an alert only after the Bus or fallback delivery succeeds;
- persists a create-only receipt beside retained alert history, preserving
  idempotency across daemon restart without mutating the source event; and
- rejects empty, overlong, and traversal-capable alert identifiers before they
  can become receipt paths, and never trusts or follows a forged receipt
  symlink.

The focused hostile test covers successful delivery followed by worker reopen,
failure of both delivery routes followed by retry, a `..` identifier that must
not create a receipt outside the bounded receipt namespace, and a forged
receipt symlink that must neither suppress delivery nor alter its target.

## Farm verification

Host: machine 196 build VM `172.20.0.196`

Slot: `func020-alert-r18`

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=func020-alert-r18 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  workers::alert_relay::tests::delivery_receipts_survive_restart_retry_failure_and_reject_hostile_ids \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,395 filtered out**. The crate emitted its
existing warning backlog; no warning originated from the changed alert relay
path.
