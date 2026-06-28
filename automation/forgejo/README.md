# FARM-AUTO-2 / DAR-20..26 — Forgejo CI on the control VM

Self-hosted Forgejo (the platform's chosen CI; GitHub stays canonical, Forgejo
pull-mirrors) as the **trigger/UX** for farm builds — push / schedule / manual,
with logs, matrix, retries. The work runs on the fleet via the shared substrate
(`automation/lib` + the reconciler), so this layer is thin glue over proven machinery.

**v2 (DAR-20..26):** Forgejo + the runner live on the **control VM**, bound to its
**overlay IP** (not LAN / `0.0.0.0`); all durable secrets come from / persist to the
**mesh secret store** (`/mcnf/secret/forgejo-*`), so a control-VM rebuild
reconstitutes CI with no re-pasting and no host-local plaintext.

## Bring it up (one-shot, on the control VM)
```sh
automation/forgejo/forgejo-deploy.sh          # DAR-26: the whole subsystem, ordered + idempotent
# or step-by-step:
automation/forgejo/forgejo-up.sh              # server + admin + runner token (overlay bind, store-backed)
automation/forgejo/forgejo-runner-up.sh       # HOST-NATIVE act_runner (systemd, label farm)
automation/forgejo/forgejo-seed.sh            # repo: GitHub pull-mirror | on-disk air-gap seed
automation/forgejo/dnf-channel-up.sh          # sovereign dnf channel (gh-pages shape, HOLD area)
```
The host overlay IP is auto-detected (nebula/mde-neb) or `--host <overlay-ip>`.

## Data model + storage

| Component                 | Storage                                   | Notes |
|---------------------------|-------------------------------------------|-------|
| Forgejo app DB            | sqlite at `$MCNF_FORGEJO_DATA/gitea/forgejo.db` (default `/var/lib/mcnf-forgejo`) | control-VM-local app state; covered by DR (DAR-38/43), NOT co-mingled with tofu state |
| `SECRET_KEY`              | `/mcnf/secret/forgejo-secret-key`         | minted+stored on first stand-up; reused on rebuild |
| admin password            | `/mcnf/secret/forgejo-admin-pass`         | minted+stored if absent; admin = `mcnfadmin` |
| runner registration token | `/mcnf/secret/forgejo-runner-token`       | re-minted each `forgejo-up.sh`, persisted to the store |
| runner identity (`.runner`)| `$MCNF_RUNNER_WORKDIR` (`/var/lib/mcnf-forgejo-runner`) | host-native; recreated by re-register |
| sovereign dnf channel     | `$MCNF_DNF_ROOT` (`/var/lib/mcnf-dnf-channel`) | `fedora-N-x86_64/repodata` + `HOLD/` + the GPG key |

The **only durable Forgejo state** is the sqlite DB + the three `/mcnf/secret/forgejo-*`
values. Everything else (runner identity, channel metadata) is regenerated from those.

## CI — `.forgejo/workflows/`
- `farm-gate.yml` — on push/schedule/manual: converge the worklist's active `@farm`
  jobs onto the fleet (`farm-reconcile.sh`).
- `rpm-publish.yml` (DAR-24) — on master push / dispatch: build the **full + server**
  RPMs via `build-rpm-fedora43.sh` on `runs-on: farm`, then `createrepo_c` them into
  the sovereign channel's **HOLD** area. **UNSIGNED** — the workflow NEVER calls the
  GPG sign step; signing stays operator-gated (`sign-release.sh` / `/release`), which
  promotes an artifact out of HOLD.

## Sovereign dnf channel (air-gap path)
`dnf-channel-up.sh` serves a gh-pages-shaped channel over the overlay
(`http://<overlay>:8480/fedora-N-x86_64/...` + `RPM-GPG-KEY-magic-mesh`). Point
`do-lighthouse-cloudinit.sh REPO_BASEURL` at it and a fresh peer dnf-installs from
the mesh with no GitHub (`gpgcheck=1` stays on).

## Notes (hard-won)
- The Forgejo container needs `:Z` on the data bind-mount (SELinux on EL9) and the
  sqlite path under the work dir (`/data/gitea/forgejo.db`). Headless install via
  `INSTALL_LOCK=true` + the `SECRET_KEY` from the store.
- The runner is **host-native on purpose** — a containerised runner can't reach the
  build VMs / etcd / the overlay, but a host runner inherits the mesh key + the
  substrate, so `runs-on: farm` steps dispatch to the fleet.
