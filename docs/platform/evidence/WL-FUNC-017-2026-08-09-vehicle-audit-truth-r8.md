# WL-FUNC-017 vehicle reboot audit truth — r8

Date: 2026-08-09

Base revision: `6d8475730fe38ce868d99b80c8bb47f3abd76fe5`

## Production correction

`events::append_and_alert` now returns `true` only after the hash-chained event
row commits. Store-open or append failure is logged and returns `false`. Alert
hooks remain best-effort and are explicitly outside that result; this evidence
does not claim hook delivery.

The vehicle reboot path preserves HMAC authorization before the live ESN probe,
typed arming before SSH, and audit persistence after successful SSH. A successful
SSH reboot with a failed audit commit remains `ok: true` with
`applied: "reboot issued"`, reports `audited: false`, and returns the bounded
detail `reboot issued, but the audit event did not commit`.

## Exact BigBoy verification

Host: XEN-BIGBOY build VM `172.20.0.130`

Slot: `vehicle-audit-truth-r8`

Committed-audit fixture:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=vehicle-audit-truth-r8 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::vehicle::tests::successful_reboot_reports_audited_only_after_event_commit \
  -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4406 filtered out`. The test reopens the
SQLite store and proves the committed row is `admin_action`, actor
`peer:rig-1`, with the exact vehicle/reboot/ESN payload before accepting
`audited: true`.

Hostile store-open failure fixture:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=vehicle-audit-truth-r8 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::vehicle::tests::successful_reboot_with_forced_audit_store_failure_is_not_fabricated \
  -- --exact --nocapture
```

Result: PASS — `1 passed; 0 failed; 4406 filtered out`. A regular file blocks
the configured database parent, so store open fails deterministically after the
fake SSH reboot succeeds. The reply remains applied and successful while
`audited` is false and the bounded audit error is exact.

Exact two-file formatting check after the final formatting-only adjustment:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=vehicle-audit-truth-r8 \
  install-helpers/xcp-build.sh sync
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes mm@172.20.0.130 \
  'cd ~/magic-mesh-farm-vehicle-audit-truth-r8 && rustfmt --edition 2021 --check \
   crates/mesh/mackesd/src/events.rs \
   crates/mesh/mackesd/src/workers/vehicle.rs'
```

Result: PASS, exit 0 with no rustfmt diff.

Scoped local `git diff --check` also passed.

## Source identity

```text
ef70c42efbaa4fb769f596ef39964a8294d5717d50829bfb5159d137407bbfa0  crates/mesh/mackesd/src/events.rs
ccd7051a4317fdb20408e6c7d9ced856b8ef8f6ace2fd65e62f23ee5fb1b24c7  crates/mesh/mackesd/src/workers/vehicle.rs
50bdb3229f1c7031bfa1c7e2b838beeba34000cdbca27a2a5fdddc7a7bfbfb12  scoped source diff
```

No commit or live MG90 reboot was performed. Concurrent dirty
`desktop_sources`, shell-health, and unrelated evidence changes were not edited.
