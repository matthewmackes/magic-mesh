# WL-FUNC-029 source panel honesty — invalid DID / unknown node / conflict — r1

Date: 2026-08-26
Observed: `2026-08-26T12:10:14Z`–`2026-08-26T12:10:27Z`
Classification: implementation / farm contract evidence; **not** live Vitelity,
**not** dest-operator, **not** installed-seat mutation, **not**
`production_admitted`
Source worktree: `agent/drain-worklist-20260725` dirty over `4151fba0b`
`production_admitted: false`

## Exact scoped result

Communications Activity already packed the Fleet voice-admin panel. This slice
makes the apply/verb boundary refuse in honest order and keeps the empty
console closed without a sealed sub-account:

- invalid DIDs refuse before an empty or unknown node identity
- unknown nodes and in-flight conflicting DID routes refuse without queuing
- the same DID pending toward the same node is not a conflict
- ambiguous hostnames refuse as unknown
- routing to a still-`Provisioning` node on a mixed board refuses
  `NoProvisionedAccount`
- cutover no longer arms confirm when `state/voice/*` has no provisioned
  account; the empty headline stays `No provisioned voice account`
- provision still publishes on an empty board (reconcile is allowed; DID /
  failover / shared / cutover are not)

Write scope:

- `crates/desktop/mde-collab-egui/src/activity.rs`

No `voice_provision.rs`, `ipc/voip.rs`, mackesd onboard, Construct seat, or
Vitelity dest. No provision click.

## Focused farm evidence

A peer already held `cargo test -p mde-collab-egui`. This unit waited on that
command lock, then admitted its own dirty-tree run (dirty results are never
reused):

```text
automation/lib/farm-dispatch.sh run WL-FUNC-029-panel-honesty-r1 \
  "cargo test -p mde-collab-egui"
```

Admission: `.130` slot `1` (`magic-mesh-farm-d1`), light shape. Result JSON:
`automation/.state/results/WL-FUNC-029-panel-honesty-r1.json` (`pass`, exit 0).
Lib suite: `193 passed, 0 failed` (0 doctests). Covered:

- `invalid_dids_unknown_nodes_and_conflicts_refuse`
- `apply_voice_admin_refuses_invalid_dids_unknown_nodes_and_conflicts`
- `voice_panel_stays_empty_without_a_provisioned_account`
- `fleet_voice_admin_apply_publishes_typed_verbs_and_empty_projections_stay_honest`

This is farm implementation/contract evidence, not live Bus or provider
round-trip.

## Leftover

- `@leftover:{live-seat}` remains Activity paint of the honest empty board on a
  current-revision seat. This unit did not occupy Construct seats and did not
  click Provision.
- `@leftover:{dest-operator}` stays parked. No Vitelity dest, master key, or
  DID was invented. Do not unpark until a named dest is authorized with the
  unpublished signed candidate plus red alert + 5s.
