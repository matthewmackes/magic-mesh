# WL-FUNC-029 / WL-FUNC-030 Activity admin evidence — 2026-08-20

## Exact scoped result

The owned Activity seam is complete in commit
`fb1a75df49435777ff6e0c1f14f6a0d2e4812f05` (`fix: complete gateway expiry admin
contract`). No additional implementation patch is required in
`crates/desktop/mde-collab-egui/src/activity.rs`.

Communications Activity is runtime-wired through
`activity_body_with_admin`: the shell supplies retained
`state/voice/*`/`get-gateway` projections, the panel renders honest empty and
freshness states, and the shell drains typed sinks onto the existing
`action/voice/*` and `action/voip/{set,get,clear}-gateway` verbs. The scoped
correctness boundary includes:

- provision, DID-route, failover, shared-outbound, and cutover admission;
- invalid DID, unknown-node, conflicting-route, unprovisioned-account, and
  cutover-not-ready refusals;
- gateway host, port, username, and REGISTER-expiry validation;
- set/get/clear gateway commands with confirmed, replay-resistant clear;
- password input clearing after set/clear and password-redacted retained
  readout/debug behavior.

The shell integration is present in
`crates/desktop/mde-shell-egui/src/communications/mod.rs`; no worker,
responder, or contract change is needed. The only pre-existing dirty path at
inspection was `crates/mesh/mackesd/src/onboard/invite.rs`, outside this
ownership scope.

## Focused farm evidence

A fresh governed farm admission found `.50`, slot `0`, below the helper's
8-GiB sync-headroom requirement (3.5 GiB free), so that attempt was refused
before sync. The same immutable focused job was then admitted on `.196`, slot
`0`, with 23.5 GiB free:

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=0 \
  install-helpers/xcp-build.sh cargo test -p mde-collab-egui activity_ --lib -- --nocapture
```

Result: `11 passed, 0 failed, 152 filtered out` in 7m 20s.

The covered assertions include Activity rendering, honest empty voice/gateway
states, retained projections, typed verb ordering, password redaction, and
gateway expiry refusal. This is farm implementation/contract evidence, not
live Bus/provider, migrated `gateway.toml`, or installed-seat evidence.

## Fold readiness

The code slice is fold-ready for WL-FUNC-029 and WL-FUNC-030. Production
acceptance remains separately blocked on the worklist's required live Bus and
provider round-trip plus unchanged migrated `gateway.toml` evidence; no live
mutation was attempted here.
