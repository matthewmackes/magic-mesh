# Release Rollback Runbook (WL-BUILD-003)

The inverse of promotion: re-point the sovereign dnf channel to the **previous
NEVRA** and converge the fleet back onto it. Promotion is documented in
[`docs/ops/promotion-pipeline.md`](ops/promotion-pipeline.md); this is the return
path when a promoted release is bad.

Tool: [`automation/promotion/mcnf-channel-rollback.sh`](../automation/promotion/mcnf-channel-rollback.sh)
(runs on the control VM, where the channel and `createrepo_c` live).

## Model

The sovereign channel serves every RPM in `fedora-<N>-x86_64/` that is not under
an excluded staging subtree; a dnf client installs the highest-version one. Two
subtrees are excluded from the client-facing index:

- `HOLD/` — DAR-24 unsigned CI staging.
- `ROLLED-BACK/` — the rollback quarantine (WL-BUILD-003).

A **rollback** moves the current newest NEVRA out of the client-facing set into
`ROLLED-BACK/` and reindexes, so the channel's advertised "latest" reverts to the
previous NEVRA. A **re-promote** moves it back. The exclusion is enforced in both
the rollback tool and `dnf-channel-up.sh` (`indexable_rpms` prune +
`createrepo_c --excludes`), so a channel refresh never re-advertises a rolled-back
RPM. Per the fleet-downgrade mandate in
[`docs/POSTMORTEM-line-divergence.md`](POSTMORTEM-line-divergence.md), a downgrade
on a production channel is an explicit, typed-confirm operation.

## When to roll back

- A promoted release regresses live behavior (crash loop, mesh/peer health,
  media, seat shell) and a fix-forward is not immediately available.
- `mcnf-promotion-cycle.sh live-smoke` / `live-audit` fails **after** a promotion
  and the previous NEVRA is known-good.
- A version-collision / downgrade guard tripped and you need the fleet back on the
  prior line while the divergence is reconciled.

Prefer fix-forward (a higher NEVRA) when a fix is ready — that is a normal
promotion, not a rollback. Roll back when you need the *known-good previous* build
serving now.

## Safety model

`mcnf-channel-rollback.sh` is **dry-run by default** — without `--apply` it prints
the plan and changes nothing. A real mutation on the production channel root
(`/var/lib/mcnf-dnf-channel`) additionally requires the typed token
`--confirm ROLLBACK`. `--non-prod` is for scratch/test roots only and refuses to
touch the production default root. The `drill` verb exercises the whole cycle on a
throwaway temp root and never touches production.

## Procedure

Run on the control VM (has the channel + `createrepo_c`).

### 1. Inspect the ladder (read-only)

```bash
automation/promotion/mcnf-channel-rollback.sh list --fedora 44
```

Confirms the current latest (CURRENT) and the rollback target (previous). Repeat
per Fedora target (e.g. `--fedora 43`).

### 2. Preview the rollback (dry-run — the default)

```bash
automation/promotion/mcnf-channel-rollback.sh rollback --fedora 44
```

Prints which NEVRA(s) would be quarantined and the resulting channel latest.
Nothing is changed.

### 3. Apply the rollback (typed-confirm on prod)

```bash
automation/promotion/mcnf-channel-rollback.sh rollback --fedora 44 \
  --apply --confirm ROLLBACK
```

To roll back multiple releases at once, target a specific NVRA — everything newer
is quarantined:

```bash
automation/promotion/mcnf-channel-rollback.sh rollback --fedora 44 \
  --to magic-mesh-12.0.0-1.x86_64 --apply --confirm ROLLBACK
```

Repeat for every Fedora target you promoted (`--fedora 43`, `--fedora 44`).

### 4. Converge the fleet

The channel now advertises the previous NEVRA. On each installed host (Eagle, DO
lighthouses, seats — operator-gated SSH):

```bash
dnf clean metadata && dnf distro-sync magic-mesh   # re-sync to the channel latest
# or pin explicitly:
dnf -y downgrade magic-mesh-<previous-version-release>
```

`distro-sync` is required because plain `dnf update` never downgrades, and the
build refuses a silent downgrade by design
([`docs/design/platform-survey-answers.md`](design/platform-survey-answers.md)
Q78).

## Verify

1. **Channel:** `mcnf-channel-rollback.sh list --fedora <N>` shows the previous
   NEVRA as CURRENT and the bad NEVRA under `ROLLED-BACK/`.
2. **Metadata:** the bad NEVRA is absent from a fresh client's view —
   `dnf clean metadata && dnf list magic-mesh --showduplicates` no longer offers
   it.
3. **Fleet:** on each host `rpm -q --qf '%{VERSION}-%{RELEASE}\n' magic-mesh`
   equals the rollback target, and
   `mcnf-promotion-cycle.sh live-smoke` / `live-audit` pass on the previous NEVRA.
4. **Services:** `systemctl is-active mackesd nebula syncthing` (and `etcd` on
   lighthouses) are active.

## Re-promote (undo a rollback)

Once the regression is understood, move the quarantined NEVRA back into the
client-facing set:

```bash
automation/promotion/mcnf-channel-rollback.sh list --fedora 44      # find the NVRA
automation/promotion/mcnf-channel-rollback.sh re-promote magic-mesh-12.1.0-1.x86_64 \
  --fedora 44 --apply --confirm ROLLBACK
```

Then converge the fleet forward with the normal promotion path
(`mcnf-promotion-cycle.sh`). Prefer shipping a **fix-forward** higher NEVRA over
re-promoting a build that was rolled back for cause.

## Non-production drill

Prove the whole promote -> rollback -> re-promote cycle and the safety guards on a
throwaway temp root — no production contact, no external deps:

```bash
automation/promotion/mcnf-channel-rollback.sh drill
```

Expected tail: `drill: ALL PASS`. The drill validates the NEVRA ladder, the
quarantine move, `--apply`/typed-confirm gating, and the `--non-prod`
production-root refusal.
