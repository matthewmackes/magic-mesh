# WL-FUNC-011 — V2 transfer no-replace admission (r195)

Date: 2026-08-10

## Gap

`V2Ledger::submit` checked whether a transfer record existed and then called
the replace-on-rename update path. Two concurrent or replayed submissions
could therefore pass the check and overwrite the first admitted record.

## Correction

`crates/mesh/mackesd/src/workers/transfers/ledger.rs` now writes the complete
temporary record first and atomically installs new submissions with a
same-directory hard link. An existing record, including a hostile final
symlink, wins the race and is mapped to the typed `Duplicate` refusal. Existing
controls retain the replace-on-rename path because they update an already
admitted identity.

## Focused farm proof

Farm host: `172.20.0.90`

Slot: `func011-v2-no-replace-r195`

Command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func011-v2-no-replace-r195 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  workers::transfers::ledger::tests::v2_ledger_admits_controls_and_survives_reopen \
  -- --exact --nocapture
```

Result:

```text
1 passed; 0 failed; 0 ignored; 0 measured; 4727 filtered out
```

The regression submits a valid row, attempts a replay with the same transfer
identity but changed update metadata, requires `Duplicate`, and verifies that
the originally admitted row remains unchanged.

## Limits

This is a durable local-ledger admission proof. It does not prove cross-node
transport delivery, executor completion, or physical-seat transfer UX.
