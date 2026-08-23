# WL-FUNC-023 leftover (1) — live enroll bearer mint — r1

Date: 2026-08-23  
Classification: leftover (1) live mint through existing lifecycle authority;
**not** leftover (2) login-env mutation, leftover (3) enroll/offboard, freeze,
or `production_admitted`  
Worklist unit: operator "create the token"  
Source revision: `de89cb277f50`  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023` leftover (1).
- Operator 2026-08-23: create the token. No enroll, offboard, or publish.

## Live authority (not invented)

Seat 15 `mackesd peers` lists five workstations and no lighthouse rows.
Nebula `config.yaml` on Seat 15 maps overlay `10.42.0.1` to underlay
`104.236.118.177:4242`. Overlay ping from Seat 15 to `.1` still fails.

`root@104.236.118.177` with the control-host identity answers as
`lh-mcnf-clean-20260728-1785239652`, role `lighthouse`, overlay
`10.42.0.1`. Founding bundle

`/mnt/mesh-storage/peer:lh-mcnf-clean-20260728-1785239652/mackesd/nebula-bundle.json`

and `/etc/mackesd/site.yml` both carry mesh-id `mcnf-clean-20260728`.
LH1 package is `magic-mesh-lighthouse-12.1.6-11`. `/usr/bin/mackesd` is a
regular file. Sibling droplets `46.101.219.245` and `64.23.131.57` are
also reachable as root.

## Warning

Unpublished signed candidate dest admits (`production_admitted: false`).
Control host has no `mde-bus`. Dell (`172.20.146.225`) ran
`install-helpers/seat-update-warning.sh` first: persist-only
`event/toast/show` with `AI-GENERATED-ALERT`, then the five-second hold.
`WARN_RC=0` at 2026-08-23T20:21Z.

## Mint

`mackesd enroll-token --mesh-id mcnf-clean-20260728 --lighthouse 104.236.118.177`
ran on LH1 after the warning. A space-split SSH probe earlier failed with
`unexpected argument 'leftover'` and minted nothing. The successful mint
wrote dest + sidecar on the control host. Helper stdout never carried
bearer or join-token bytes.

| Field | Value |
|---|---|
| dest | `/root/mcnf-private/enroll-bearer` mode `0600` 43 bytes URL-safe |
| sidecar | `/root/mcnf-private/enroll-bearer.json` mode `0400` |
| sidecar kind | `mcnf-enroll-bearer-mint` |
| `bearer_sha256` | `09da661cd4b22c829320b9d7c473db156c94101a61f1a7e77269feabcbcd5d36` |
| mesh-id | `mcnf-clean-20260728` |
| LH ledger name | `/mnt/mesh-storage/ca/issued-bearers/<same sha256>.json` |
| `production_admitted` | false |
| `enroll_succeeded` | false |

Sidecar body does not contain dest bytes or a `mesh:…#…` token. Login env
still has `JOIN_TOKEN` and `MACKESD_BOOTSTRAP_SSH_KEY` unset. Existing
bootstrap dests were not replaced.

## Non-claims

- Live enroll, join, offboard, reenroll, wipe, and package install were
  not attempted.
- `production_admitted` was not flipped.
- Seat 15 remains an enrolled workstation; it is not a fresh-box target.
- Overlay from Seat 15 to LH1 is still down; leftover (3) still needs a
  reachable enroll path and operator offboard+reenroll if that seat is
  the target.
- Leftover (2) child-only dest-env runner is unchanged.

## Blocker

`WL-FUNC-023` stays `Remaining`. Leftover (1) dest exists. Freeze bar is
still leftover (2) and leftover (3).
