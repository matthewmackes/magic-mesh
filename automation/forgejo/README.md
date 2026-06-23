# FARM-AUTO-2 — Forgejo Actions drives the build farm

Self-hosted Forgejo (the platform's chosen CI; GitHub stays canonical, Forgejo
pull-mirrors) as the **trigger/UX** for farm builds — push / schedule / manual,
with logs, matrix, retries. The work runs on the fleet via the shared substrate
(`automation/lib` + the reconciler), so this layer is thin glue over proven machinery.

## Bring it up (control host)
```sh
automation/forgejo/forgejo-up.sh --admin-pass '<pw>'   # server + admin + runner token (podman :3000)
automation/forgejo/forgejo-runner-up.sh                # HOST-NATIVE act_runner (systemd)
```
The runner is **host-native on purpose** — a containerised runner can't reach the
build VMs / etcd / XO, but a host runner inherits the mesh key + the substrate, so
`runs-on: farm` steps dispatch to the fleet.

## Wire the repo
```sh
# create the repo in Forgejo (API or UI), then:
git remote add forgejo http://<host>:3000/mcnfadmin/magic-mesh.git
git push forgejo master           # triggers .forgejo/workflows/farm-gate.yml
```

## The workflow — `.forgejo/workflows/farm-gate.yml`
On push to master / every 30 min / manual: checkout → show farm node state →
`farm-reconcile.sh` (converge the worklist's active `@farm` jobs onto the fleet) →
publish status. Same substrate as FARM-AUTO-4, fronted by Forgejo's triggers + UI.

## Notes (hard-won)
- The Forgejo container needs `:Z` on the data bind-mount (SELinux on EL9) and the
  sqlite path under the work dir (`/data/gitea/forgejo.db`) — both baked into
  `forgejo-up.sh`. Headless install via `INSTALL_LOCK=true` + a persisted secret.
- Ephemeral-VM-per-job autoscaling (a runner that `tofu apply`s a fresh VM per job)
  is the advanced form; today the host runner dispatches onto the standing 3-node
  pool, which already packs jobs across nodes via the substrate's per-node flock.
