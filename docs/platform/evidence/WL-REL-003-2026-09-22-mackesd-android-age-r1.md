# WL-REL-003 mackesd farm unit — Android observation age — r1

Date: 2026-09-22  
Classification: focused farm-unit repair; **not** Android enablement, dest
invention, publication, or S4 close  
`published: false`  
`production_admitted: false`

## Failed gate

`cargo test -p mackesd` on `172.20.0.90` at `36d8c69e8-dirty` exited 101.

```
workers::cloud::tests::admitted_android_inventory_replaces_pending_and_replay_cannot_rollback
assertion `left == right` failed
  left: Unavailable
  right: Booting
```

`build_state()` projects retained Android inventory through
`project_android_inventory_age`. The fixture used a fixed
`observed_at_unix_ms` of `1_786_000_000_000` (2026-08). Today that stamp is
older than `MAX_ANDROID_OBSERVATION_AGE_MS` (30 days), so the mirror correctly
becomes `Unavailable` / `ObservationStale`. Android/Cuttlefish stays deferred
beyond `13.0.0`; this is not a provider restore.

## Repair

The test now admits a wall-clock-relative observation and still proves
newer-wins plus replay refusal. Other `1_786_000_000_000` fixtures that pass
an explicit `now` into `project_android_inventory_age` were left unchanged.

## Farm

Host: `172.20.0.90` (`mcnf-build-kvm-xcp1`), slot `2`,
`MCNF_BUILD_SHAPE=small`.

```
./install-helpers/xcp-build.sh cargo test -p mackesd -- \
  workers::cloud::tests::admitted_android_inventory_replaces_pending_and_replay_cannot_rollback
```

Result: the named unit `ok`. Official `cargo test -p mackesd` (no filter)
re-runs via tick-fill after this commit. XEN-BIGBOY `.130` was unreachable
and was not used.
