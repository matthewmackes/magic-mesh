# WL-FUNC-029 leftover honesty — Surface Fleet voice-admin observe — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:55Z`–`2026-08-25T10:59:30Z`  
Classification: leftover-honesty / live-seat observe without Vitelity;
**not** provision, **not** DID-route / failover / shared-outbound /
cutover, **not** dest-operator, **not** `production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`)  
`production_admitted: false`

SSH as `root@` with `/root/.ssh/mackes_mesh_ed25519`. No Vitelity dest.
No `action/voice/provision`. No master-key invent. Did not SSH Seat 15
or Dell. `@leftover:{dest-operator}` stays parked.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-029`.
- Farm Activity contract (fixtures do not close live Vitelity):
  `WL-FUNC-029-030-2026-08-20-activity-admin-farm-r2.md`.
- Awaiting-master-key honesty (source, not live provider):
  `WL-FUNC-029-2026-08-23-awaiting-master-key-honesty-r1.md`.

## Packed panel (installed Construct)

`/usr/bin/mde-shell-egui` dest-cut `4071ed295` contains:

| Literal | Count |
|---|---|
| `Fleet voice` | 1 |
| `Awaiting Vitelity master key` | 1 |
| `No state/voice nodes projected` | 2 |
| `Provision / Re-provision` | 1 |
| `Shared outbound` | 1 |

`voice_unprovisioned_headline` stays closed until a node is `Registered`
or `Unregistered`. Empty `state/voice/*` is the honest workstation
observe without a sealed master key — not a missing Activity section.

## Live Bus (observe only)

`mackesd-integrations` active. Heartbeat
`state/mackesd/integrations/workers/voice_provision` is `running` on
`peer:SURFACE` (generation advanced through the probe). sqlite topics
matching `voice` / `voip` / `transfer`:

| Topic | Count |
|---|---|
| `state/mackesd/integrations/workers/voice_provision` | 269 (heartbeat) |
| `action/voip/get-gateway` | 1 (FUNC-030 GET; not a voice verb) |
| `state/voice/*`, `state/voice-dids`, `state/voice-shared`, `state/voice-cutover`, `action/voice/*` | **absent** |

`voice_provision::reconcile_and_publish` is leader-only. Surface is a
workstation. Empty desired enrollment publishes no invented node rows.
`mde-bus history state/voice/peer:SURFACE` is empty.

Ctrl+J opened Communications Transfers (FUNC-028). Activity auto-GET did
not fire. The Fleet voice panel was therefore not painted this unit.
Packed bytes plus honest-empty Bus is the Surface observe.

## Non-claims

- Provision / Re-provision was not clicked.
- No DID, failover, shared-outbound, or cutover verb was published.
- No Vitelity account, DID, or master key was present or invented.
- Activity GUI paint of "No provisioned voice account" was not read from
  pixels (tiled XR30).

## Leftover / blocker

`@leftover:{live-seat}` on Surface is still Activity paint of the honest
empty board. `@leftover:{dest-operator}` remains parked until a named
Vitelity dest is authorized with the unpublished signed candidate plus
red alert + 5s. Do not invent that dest. Do not flip
`production_admitted`.
