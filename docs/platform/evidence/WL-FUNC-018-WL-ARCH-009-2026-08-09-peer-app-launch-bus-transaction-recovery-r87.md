# WL-FUNC-018 / WL-ARCH-009 — Peer-app launch Bus transaction recovery (r87)

Date: 2026-08-09

Farm: machine9 `172.20.0.50`, slot `peer-app-launch-bus-r87`

## Production correctness model

Owned production source:
`crates/mesh/mackesd/src/workers/peer_app_launch.rs`.

- Each poll resolves the live Bus root again and opens a fresh `Persist`
  connection between device/inode observations of `index.sqlite`. Reads,
  result writes, activation, cursor commit, and result-delivery commit all
  verify that the path still names that connection's accepted identity.
- First activation and every replacement generation use bounded
  `read_latest(action/apps/launch)` tail capture. Retained launch commands are
  never replayed; the first command appended after that exact tail remains
  forward work. A same-path swap during activation or a page read rejects the
  retired transaction without advancing the cursor or reaching authorization
  or launch.
- Forward reads use `list_since_limit(..., 32)`. The complete bounded page is
  materialized and identity-checked before any cursor, replay-ledger, journal,
  or process effect. Bodies are capped at 64 KiB before durable admission.
- The host-local journal is outside the replaceable Bus generation. It binds
  the exact wire body, target node, catalog id, and locally resolved argv before
  capability consumption. The existing exact-body HMAC, 30-second lifetime,
  single-use nonce, target-node, XDG catalog, source/mode, and no-wire-command
  checks remain authoritative.
- Before `Command::spawn`, the worker fsyncs an `effect_claimed` phase. A
  recovered prepared/claimed record is converted to typed `indeterminate` and
  is never launched. An error returned after the claim is also
  `indeterminate`, never guessed to be failure or success and never retried.
  Only a successful spawn return produces `succeeded` with reason
  `launch-spawned`.
- Every terminal refusal, success, or indeterminate outcome is fsynced before
  publication on `reply/<action-ulid>`. Delivery is bound to Bus device/inode.
  Exact-body lookup suppresses a duplicate after write-before-receipt crash;
  replacement after a write leaves the durable record pending, and activation
  republishes it into the current index without repeating the launch.
- Journal reads are no-follow, regular-file, 4 MiB/2,048-record bounded.
  Writes use a mode-0600 no-follow temporary file, file fsync, atomic rename,
  and parent-directory fsync. Capacity reclamation removes only the oldest
  terminal record already delivered to a Bus generation; undelivered or
  ambiguous authority fails closed.

## Exact hostile farm verification

The first attempted Cargo command named four positional filters. Cargo rejected
that invocation before compilation because it accepts one filter. The four new
tests were then given the common `bus_r87_` prefix and the exact scoped gate was
run:

```text
MCNF_BUILD_HOST=172.20.0.50 \
MCNF_BUILD_SLOT=peer-app-launch-bus-r87 \
install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  bus_r87_ -- --nocapture
```

Result: PASS. The full `mackesd` library test target compiled and linked in
7m01s; `4 passed; 0 failed; 0 ignored; 4,614 filtered out` in 0.13s.

```text
bus_r87_retained_action_is_skipped_and_first_forward_action_launches_once ... ok
bus_r87_same_path_replacement_after_read_retires_page_and_preserves_first_forward ... ok
bus_r87_replacement_after_result_write_corrects_forward_without_repeating_launch ... ok
bus_r87_recovered_effect_claim_and_spawn_error_are_indeterminate_never_success ... ok
```

The cases prove retained-tail suppression plus first-forward execution; swap
after complete bounded read with zero retired effects; swap after launch/result
append with one launch and current-index corrected-forward success; and restart
recovery/error truth as indeterminate with no repeat or false success.

The tested pre-format source SHA-256 was:

```text
bcc34a34c79ed2d8152440937e28eb8647ddee237cf6caa0a3775243b4feec14
```

`rustfmt --check` then identified formatting-only differences. They were
applied with no logic change. The synchronized final source passed the scoped
farm check:

```text
ssh mm@172.20.0.50 \
  'cd /home/mm/magic-mesh-farm-peer-app-launch-bus-r87 && \
   sha256sum crates/mesh/mackesd/src/workers/peer_app_launch.rs && \
   rustfmt --edition 2021 --check \
     crates/mesh/mackesd/src/workers/peer_app_launch.rs'
# exit 0
# 667c1e343deda8eb6bc58ba3f06257f46fc8c9674fa71d9368260f39c357713e
```

The post-format focused Cargo rerun was attempted. It was blocked before the
r87 tests by four new `unused Result` errors in concurrently modified,
out-of-scope `crates/mesh/mackesd/src/workers/compute_expose.rs` lines
1922/1956/1985/2039. That file was not changed or reverted here. This does not
replace the successful full-library compile/test above; it records why there is
no second test claim for the formatting-only final bytes.

Local final checks:

```text
git diff --check -- crates/mesh/mackesd/src/workers/peer_app_launch.rs
# exit 0

sha256sum crates/mesh/mackesd/src/workers/peer_app_launch.rs
# 667c1e343deda8eb6bc58ba3f06257f46fc8c9674fa71d9368260f39c357713e

git diff -- crates/mesh/mackesd/src/workers/peer_app_launch.rs | sha256sum
# 1e3e0c7bf0310df4dfd9860d20fd8db40502f88b9df03aec497d2111115b1562
```

## Residual live-proof gaps

- The focused tests use an injected launcher and temporary SQLite generations.
  No installed workstation, real desktop process, Front Door publisher, or
  cross-peer UI/result rendering was exercised.
- `Command::spawn` and the journal cannot be one atomic primitive. Claim-first
  recovery deliberately reports indeterminate and refuses to retry when death
  or an adapter error makes the external effect unknowable; an operator may
  need to inspect the process/session state.
- Bus append, identity validation, and journal receipt are separate operations.
  Exact-body lookup suppresses ordinary write-before-receipt duplication, but
  replacement can always occur immediately after a validation. The next fresh
  transaction detects a different identity and republishes the durable result;
  the launch claim prevents effect repetition.
- No WORKLIST edit, unrelated-file correction, commit, or push was performed.

## Final hashes

```text
667c1e343deda8eb6bc58ba3f06257f46fc8c9674fa71d9368260f39c357713e  crates/mesh/mackesd/src/workers/peer_app_launch.rs
```
