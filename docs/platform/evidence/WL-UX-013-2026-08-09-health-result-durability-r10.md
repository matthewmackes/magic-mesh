# WL-UX-013 S4 health action result durability — r10

- Date: 2026-08-09
- Base commit: `1c1c009f3ad9a8565e6e950084b6964084f5974b`
- Farm host: machine 193 (`172.20.0.90`)
- Farm slot: `health-result-durability-r10`
- Source SHA-256: `852872b45e409fe46f52eae4007a9b7c2b59f72d09bf721252263fd5dc8fb7b4`

## Production correction

`crates/mesh/mackesd/src/workers/node_grade.rs` no longer consumes an exact
health action before it can account for the terminal result. An authorized
mutation first writes an execution claim to the host-local root authority at
`/var/lib/mackesd/node-grade-action-results`; terminal results replace that
claim before the action cursor advances. A retained terminal result is
published without repeating remediation. A retained claim is converted to an
explicit failed/indeterminate typed result and is never executed again.

The state root is local, not QNM-Shared. Production admits only a real
root-owned `0700` directory below the trusted `/var/lib/mackesd` boundary and
root-owned `0600` regular records. Symlinks, unsafe owner/mode, oversized
records, malformed source ULIDs, and unexpected entries fail closed. File data
is synced before rename, and the state-root directory is synced after claim or
terminal rename, stale-temp cleanup, and terminal deletion. The pending set is
bounded at 128 using a 129th-entry rejection; safe temporary records share that
bound and are removed durably. Write/sync/rename failures clean their exact
safe temporary record where possible.

Target, condition, action, and expected snapshot generation authorization are
unchanged. Worker boot still starts the Bus action cursor at the retained tail,
after correcting any host-local durable result forward.

## Focused farm proof

The source was synced with:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=health-result-durability-r10 \
install-helpers/xcp-build.sh sync
```

Each focused test then ran in
`/home/mm/magic-mesh-farm-health-result-durability-r10` with the corresponding
exact command:

```text
cargo test -p mackesd --features async-services --lib \
  workers::node_grade::tests::terminal_result_storage_failure_recovers_without_repeating_mutation \
  -- --exact --nocapture

cargo test -p mackesd --features async-services --lib \
  workers::node_grade::tests::result_publication_failure_replays_durable_result_without_repeating_mutation \
  -- --exact --nocapture

cargo test -p mackesd --features async-services --lib \
  workers::node_grade::tests::local_action_journal_rejects_symlink_and_unsafe_owner_or_mode \
  -- --exact --nocapture

cargo test -p mackesd --features async-services --lib \
  workers::node_grade::tests::local_action_journal_enforces_cap_and_cleans_bounded_safe_temp \
  -- --exact --nocapture

cargo test -p mackesd --features async-services --lib \
  workers::node_grade::tests::applied_actions_emit_audited_results_with_refreshed_evidence \
  -- --exact --nocapture
```

Final results:

```text
terminal_result_storage_failure_recovers_without_repeating_mutation ... ok
result_publication_failure_replays_durable_result_without_repeating_mutation ... ok
local_action_journal_rejects_symlink_and_unsafe_owner_or_mode ... ok
local_action_journal_enforces_cap_and_cleans_bounded_safe_temp ... ok
applied_actions_emit_audited_results_with_refreshed_evidence ... ok

Each command: 1 passed; 0 failed; 0 ignored; 0 measured; 4402 filtered out
```

The exact source format check also passed on machine 193:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/node_grade.rs
```

An unrelated concurrent dirty edit in `desktop_sources.rs` did not compile and
was outside this task's permitted scope. For the final focused commands only,
the farm copy of that one unrelated file was restored from committed HEAD after
sync; the local dirty file was never edited or reverted. The exact current
`node_grade.rs` above remained synced and is bound by its SHA-256.

No broad or full tests were run. Scoped `git diff --check` passed.
