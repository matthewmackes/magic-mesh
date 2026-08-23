# WL-FUNC-029 fleet voice-admin leftover honesty — awaiting master key — r1

Date: 2026-08-23
Classification: implementation / farm contract evidence; **not** live Vitelity
or installed-seat mutation
Source worktree: `cursor-WL-FUNC-029-7ab1f91467f0`
`production_admitted: false`

## Exact scoped result

`voice_provision::awaiting_master_key` publishes a hostname-derived username
and SIP URI in `Provisioning`. The Activity panel treated any non-empty
username as a provisioned sub-account, which would unlock DID routing /
failover / shared-outbound / cutover against an empty live inventory.

This slice aligns `VoiceNodeProjection::is_provisioned` with the worker
(`Registered` | `Unregistered` only), names the leftover
("Awaiting Vitelity master key" / "Voice provider error"), and resolves
hostname or unique username to the worker `node_id` so published verbs stay
on the unchanged contract.

Write scope (disjoint from `mackesd` / `voice_provision.rs` and from
WL-FUNC-030 gateway hunks except shared `activity.rs` file):

- `crates/desktop/mde-collab-egui/src/activity.rs`
- `crates/desktop/mde-collab-egui/src/tests.rs`

No worker, secret-store, or Bus contract change. No seat mutation.

## Focused farm evidence

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-egui
```

Admission: `.90` slot `1`, 74.9 GiB free (required 8 GiB). Result:
`184 passed, 0 failed` in the lib suite (plus 0 doctests).

Covered: awaiting-master-key / IntegrationGated rows stay closed; hostname
and username resolve to `peer:<host>` on the published verb; render paints
"Awaiting Vitelity master key" and keeps DID/failover hidden; existing
provisioned verb round-trips unchanged.

## Leftover

Production acceptance remains live Vitelity on a current-revision seat
(operator lock: unpublished signed candidate + red alert + 5s). This
record is not that evidence.
