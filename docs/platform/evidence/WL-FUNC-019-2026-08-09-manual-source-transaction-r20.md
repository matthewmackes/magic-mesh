# WL-FUNC-019 — transactional manual desktop sources (r20)

Date: 2026-08-09

Working-tree base revision: `6d8475730fe38ce868d99b80c8bb47f3abd76fe5`

Final production source SHA-256:
`53152832aaf3170e4cbdc3753d4c104fca62ef02adcf004c97888587b72c96ea`

Scoped source-diff SHA-256:
`0023d7914497e992d5d29b373f9df66db56604f39b1122048ec61b03d013e057`

Farm lane: machine 9 (`172.20.0.50`), slot
`desktop-source-transaction-r20`

## Correction

Manual RDP/VNC source mutations now clone a bounded candidate, persist that
candidate through a create-new temp file plus atomic rename, and only then
replace the worker's in-memory roster. A failed temp write or pre-rename error
therefore returns `changed=false`, keeps the last-good projection, and cannot
publish a source addition or removal that disappears after restart. Duplicate
adds and absent removals remain honest no-ops.

The persistence path rejects serialized stores above the existing 1 MiB read
bound, syncs the temp file and parent directory, and performs one bounded temp
cleanup attempt on failure without recursively removing a hostile object.

A post-rename directory-sync error is handled separately: the worker strictly
re-reads the bounded final file and commits memory only when the decoded value
exactly equals the candidate. This strict read does not use the public
corrupt-to-empty fallback, so a corrupt file cannot masquerade as a successful
removal-to-empty.

Authorization and action-cursor behavior are unchanged.

## Focused verification

Exact-file formatting passed:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/desktop_sources.rs
exit 0
```

The exact hostile tests passed:

```text
cargo test -p mackesd --features async-services \
  workers::desktop_sources::tests::add_persistence_failure_keeps_last_good_then_corrects_forward \
  -- --exact --nocapture
1 passed; 0 failed; 4405 filtered out

cargo test -p mackesd --features async-services --lib \
  workers::desktop_sources::tests::remove_persistence_failure_keeps_last_good_then_corrects_forward \
  -- --exact --nocapture
1 passed; 0 failed; 4406 filtered out

cargo test -p mackesd --features async-services --lib \
  workers::desktop_sources::tests::post_rename_error_reconciles_exact_visible_removal \
  -- --exact --nocapture
1 passed; 0 failed; 4406 filtered out
```

The add fixture occupies the temp path with a hostile directory, proves memory
and strict reload retain the empty last-good roster, repairs the path, and then
adds successfully. The remove fixture injects a pre-rename failure, proves the
source remains in memory and on reload, then removes successfully. The final
fixture performs the rename and injects the subsequent error, proving the exact
visible empty candidate is reconciled into memory and reload without ambiguity.

An attempted package-wide `cargo fmt -p mackesd -- --check` was not counted: it
reported unrelated pre-existing formatting drift in concurrent files. No broad
suite or live-seat test was run for this bounded persistence invariant.
