# Session Broker late/replaced Bus recovery r71

Date: 2026-08-09

Worklist: `WL-ARCH-010`, `WL-ARCH-009`, `WL-CRIT-007`

## Correction

`SessionBrokerWorker` now resolves its explicit/current/system Bus root on every
poll instead of freezing the production resolver once at worker startup. Each
successful open binds the transaction to the current `index.sqlite` device and
inode, so a late Bus or same-path replacement recovers without daemon restart.

The first available index retains the established durable session-log behavior:
the worker folds its complete authenticated action history before leader
convergence. A later replacement is a new transient ingress boundary. The
worker stages both the `action/vdi/session` and App VM runtime-evidence tails,
rechecks the exact index identity, and only then commits both cursors and the
new identity. Existing replacement rows cannot replay lifecycle mutations or
stale guest transitions against the already-converged roster; the first action
written after activation is admitted and converged normally.

An unavailable or partially activated Bus still defers convergence. It never
becomes an empty desired roster and therefore cannot remove live sessions from
the shared session store.

## Focused farm verification

Host: machine196 (`172.20.0.196`)

Slot: `session-bus-r71`

Source SHA-256:
`245c09d671de68d3a3c3f8cbb171c385bbae8f9d402b7812ef529bf0e441094a`

The isolated detached worktree contained commit `a290bba2` plus only the
`session_broker.rs` correction. The exact late/replacement test initially
exposed an illegal test transition (`Requested` directly to `Disconnected`);
the state machine correctly refused it. The proof was corrected to use the
legal forward `Requested` to `Active` transition.

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=session-bus-r71 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  --features async-services --lib \
  workers::session_broker::tests::late_and_replaced_bus_preserves_roster_skips_retained_and_applies_forward \
  -- --exact --nocapture

Result: PASS — 1 passed, 0 failed, 4558 filtered out.
```

The same running worker recovered from an initially blocked Bus, folded and
converged one late Open, preserved that live row across a replacement containing
a retained Close, and converged one forward Active action from the replacement.

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=session-bus-r71 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  --features async-services --lib \
  workers::session_broker::tests::unavailable_bus_defers_convergence_without_removing_live_sessions \
  -- --exact

Result: PASS — 1 passed, 0 failed, 4558 filtered out.
```

Farm `rustfmt --edition 2021 --check` identified two layout-only differences;
those exact canonical layouts were applied locally. Scoped `git diff --check`
then passed. The crate emitted its existing warning set; no warning was hidden
or treated as test evidence.

## Residual boundary

Session actions retain their existing short-lived authenticated capability and
shared-store convergence semantics. This checkpoint does not claim a
cross-resource atomic commit between the Bus log and the mesh session store.
After an index replacement, already-present rows are deliberately treated as
retained ingress and skipped; current live state remains the converged session
roster, while newly published rows are forward work.
