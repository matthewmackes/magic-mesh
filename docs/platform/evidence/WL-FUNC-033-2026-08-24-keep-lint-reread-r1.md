# WL-FUNC-033 — keep `own_nebula_ip` reconfirm (2026-08-24)

Operator authorized completing remaining worklist leftovers, including
standing keep gates. No dest invented. No PBX stack reintroduced.

## Keep lint

`install-helpers/lint-func033-keep.sh` on control-host tree at
`7fe8fad6c` (dirty mackesd join/lifecycle files unrelated and unused by
this gate):

```
lint-func033-keep: PASS: own_nebula_ip kept with callers; crates/packaging have no live PBX spawn
```

`pub fn own_nebula_ip` remains in `crates/mesh/mackesd/src/voip_rtt.rs`.
Live callers include `cli/leave.rs`, `cli/ca.rs`, `bin/mackesd/spawn.rs`,
`workers/mdns_relay.rs`, `workers/nebula_csr_watcher.rs`,
`workers/nebula_enroll_listener.rs`, `telemetry/mod.rs`, and
`ipc/nebula.rs`. `crates/` and `packaging/` have no live
`mde-voice-config` / `kamailio-mde` / `rtpengine-mde` spawn.

The keep lint remains in `ci-gate` `POLICY_LINTS`. That is the standing
invariant, not an open product leftover.

## Prior closures this leftover depended on

- Fleet-negative (no kamailio/rtpengine on Dell, Seat 15, Surface):
  `WL-FUNC-033-2026-08-22-fleet-negative-reread-r1.md`
- Operator Q9 signoff 2026-08-22; stack deleted; ledger retire rows cite
  the deleting revisions.

## Result

S1–S3 of `WL-FUNC-033` are complete. The keep symbol is present with
callers. This leftover does not require further source deletion.
