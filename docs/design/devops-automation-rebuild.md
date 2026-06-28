# DevOps Automation Rebuild — DEVOPS-AUTOMATION-REBUILD (v2)

**Status: LOCKED**
**Epoch:** 11.0 / SUBSTRATE-V2
**Owner:** chief architect (assembled from 10 per-subsystem designs; v2 resolves the NEEDS-REVISION critique + the live-infra/MIG corrections)
**Supersedes the hand-built LAN control node at 172.20.145.192 with a reproducible, portable, per-mesh DevOps backoffice carried on a dedicated control VM.**

> **v2 revision note.** Verified against live code (worktree `calm-ray-dcr8`): `nebula_enroll.rs:750` gates `ca_key_pem` on `is_lighthouse_bearer` (role-scoped bearer note), so the #12 channel does NOT reach a `--role server` peer; `seal_bytes(passphrase,&[u8])` is passphrase-based; `MCNF_ETCD` defaults to the dead `http://172.20.145.192:2379` across 6 scripts; `/etc/mackesd/etcd-endpoints` is the live source of the quorum (written by `setup-etcd.sh`, parsed by `substrate/etcd.rs::endpoints_from_file`); `xen-xapi/build-vms.tf` is 3 hardcoded adopt-only resources (not `for_each`); only `xen-xapi` + `zone1-do` have `backend.tf` (both literal `.192`), `edgeos` is LOCAL state; golden-template names disagree (`MDE-VM-golden-tc` in `variables.tf`/`enable-autoscale-timer.sh` vs bare `MDE-VM-golden` in `build-vms.tf`/`setup-xcp-golden-template.sh`); `mcnf-secret.sh` encrypts to a SINGLE recipient `age -r` published at `/mcnf/age-recipient`. The v2 design below removes every one of those contradictions.

---

## 0. Purpose & Scope

Make the MCNF DevOps backoffice **reproducible** (comes along when founding a new Nebula on a new XCP-NG machine) and **reconstitutable** (can rebuild the current hand-stood-up 172.20.145.192 setup from the live mesh + DR). The backoffice is the IaC/CI/build/DR control plane: OpenTofu state backend, mesh secret store, the four Tofu roots, and — at Full tier — self-hosted Forgejo CI, the continuous reconciler/autoscaler, the travelling sccache build farm, and disaster recovery.

The deploy unit is a **single dedicated control VM** per mesh, on the founding XCP-NG dom0, enrolled as a full mesh peer (`--role server`). Genesis opts in with `mackesd found --with-backoffice[=minimal|full]`.

**In scope:** the control VM tofu root, the genesis flag, the one orchestrator that sequences the existing bring-up scripts, de-hardcoding the LAN IP/prefixes, tier model, etcd-backed state migration (incl. edgeos), DR v2, the build-VM `for_each` port WITH `moved{}` migration, the on-VM secret-zero mechanism, and the live reconstitution.

**Out of scope (follow-ups):** control-VM HA (the founding dom0 is a SPOF; the ha-shadow-master pattern may apply later); clustering etcd beyond the current quorum; multi-region build farms; signed-value MAC on secret writes.

---

## 1. The 12 Survey Locks (authoritative)

| # | Lock | How honored in this design |
|---|------|----------------------------|
| 1 | Deploy unit = a dedicated control VM (state-backend + Forgejo + reconciler + secrets) | New `infra/tofu/control-vm/` tofu root creates ONE VM; teardown = its destroy. |
| 2 | Tofu state bootstrap = reuse the mesh etcd | State backend (`tofu-state-etcd.py`) points at the **founder etcd quorum on the lighthouses**, sourced from `/etc/mackesd/etcd-endpoints` (NEVER `.192`); etcd predates Tofu, breaking the cycle. |
| 3 | Genesis hook = a `mackesd found --with-backoffice` flag | Flag records intent to `/mcnf/backoffice/intent`, provisions the control VM, invokes `backoffice-up.sh`; default OFF. |
| 4 | Optionality = TIERED (Minimal vs Full) | One artifact, two manifests (`manifest.minimal.toml` / `manifest.full.toml`); a single tier gate in the orchestrator. |
| 5 | Control VM host = the founding XCP-NG dom0 | `infra/tofu/control-vm/` aliased `xenserver` provider aimed at the founding dom0; clones the canonical golden template (§2.8, name reconciled). |
| 6 | Control VM mesh membership = full mesh peer (overlay IP) | Headless `mackesd join <token> --role server` in cloud-init; all backoffice endpoints bind the control VM's **overlay IP**, not LAN, not 0.0.0.0. |
| 7 | etcd isolation = same quorum, separate key prefix | **CANONICAL PREFIXES (resolved drift):** Tofu state `/tofu/state/*` + locks `/tofu/lock/*` (LIVE code); secrets `/mcnf/secret/*`; age recipients `/mcnf/age-recipients/*` (per-recipient set; the legacy single `/mcnf/age-recipient` is retained for back-compat); backoffice bookkeeping `/mcnf/backoffice/*`; reconciler state `/reconciler/*`; site facts `/mcnf/site/*`. |
| 8 | Secrets bootstrap = mesh secret store, unsealed on deploy | **RESOLVED (see §2.3):** the control VM **generates its OWN age identity at first boot** (private key never leaves the VM), registers its recipient at `/mcnf/age-recipients/<node-id>`, and an operator-run (or leader-run) **re-seal** step multi-recipient-encrypts every `/mcnf/secret/*` value to that recipient. There is NO secret-zero passphrase in tofu state. The `mcnf-secret.sh get` path then resolves every cred from etcd using the VM's own key. |
| 9 | Tier contents — Minimal: state-backend + secrets + Tofu roots; Full: + CI + reconciler + build farm + DR | Encoded in the two manifests + the orchestrator phase gate (Minimal stops after Phase 3). |
| 10 | CI/git = self-host Forgejo + runner on the control VM (Full) | Generalized `forgejo-up.sh` + host-native `forgejo-runner-up.sh` targeting the control VM overlay IP; tokens from the secret store. |
| 11 | Build farm = the backoffice provisions it (Full) | Reconciler/Tofu (`infra/tofu/xen-xapi/`) stands up the build-farm VMs from a shape model; the canonical golden template carries the baked toolchain. |
| 12 | Reconciler = continuous loop, systemd-managed on the control VM (FA_APPLY-safe) | Two distinct timers (5-min autoscale, 15-min @farm build) installed PLAN-ONLY at genesis; FA_APPLY=1 armed only by explicit operator action. |

---

## 2. Per-Subsystem Locks + Architecture

### 2.1 control-vm (the deploy unit)
**Locks:**
- The control VM is created via a NEW `infra/tofu/control-vm/` root that REUSES the `xen-xapi` aliased-provider pattern (one `xenserver` provider aimed at the founding dom0) + clones the canonical golden template. It is NOT folded into `xen-xapi/build-vms.tf` (which is adopt-only) — a separate root keeps the create-with-seed lifecycle and the 0-add/0-destroy farm plan isolated.
- Bootstrap is a two-phase chicken-and-egg break: **Phase A** (off-VM, on the founding dom0/control node) brings up the state-backend + secret store against the **founder etcd quorum** (endpoints from `/etc/mackesd/etcd-endpoints`) just long enough to apply `control-vm/`; **Phase B** the VM boots, self-enrolls, and the long-lived state-backend/reconciler RELOCATE onto the VM reached over the overlay. (Honors lock 2 without a circular dependency.)
- **Phase A precondition (resolves GAP 3):** the founding dom0 (or whatever LAN node runs Phase A) MUST be an enrolled overlay member, because the founder etcd binds the lighthouse OVERLAY IP and is reachable only over Nebula. Phase A refuses to proceed unless the runner has a nebula iface with a route to a quorum member (`PHASE 0` probe). On a brand-new mesh, this means the founding dom0 runs `mackesd join` (or `found` makes it a peer) BEFORE `state-backend-bootstrap.sh` — encoded as an explicit precheck, not an assumption.
- The VM enrolls headless via `mackesd join <token> --role server` in cloud-init (token resolved from the secret store at apply), then `mackesd converge` against a generated `/etc/mackesd/site.yml` enables the tier's units, and `setup-etcd.sh --client-only --anchors <quorum-overlay-ips>` writes the VM's own `/etc/mackesd/etcd-endpoints`.
- `backoffice_tier = minimal|full` is one tofu variable driving which systemd units `converge` enables.
- `backoffice down` is a tofu destroy of `control-vm/` PLUS an explicit pre-destroy drain (DR snapshot → repoint backend to founder etcd → peer-revoke → destroy); live-gated; never auto-fires the reconciler apply.

**Architecture:** `infra/tofu/control-vm/{providers,backend,vm,variables,outputs}.tf` + `cloud-init/control-vm.yaml.tftpl` (derived from `build-vm.yaml.tftpl`, reusing the NetworkManager static-IP keyfile fix verbatim; runcmd order: render NM keyfile → `mackesd join --role server` → **`mcnf-secret.sh init-self` (generate the VM's own age key + register recipient)** → `mackesd converge` → `setup-etcd.sh --client-only`). **No sealed age key is written via write_files and no unseal passphrase is templated** — the VM keygens its own identity (resolves GAP 1/6, CONTRADICTION 1, STUB 1). Sized 4 vCPU/8 GiB/60 GiB (Minimal) or 8/16/120 (Full). Backend `http://<state-backend-host>:8390/state/control-vm`. Observability via `backoffice-status.sh --json` + `mcnf-backoffice-status.{service,timer}` mirroring `mesh-status`.

### 2.2 tofu-state (Minimal-tier floor)
**Locks:**
- Honor the LIVE prefix `/tofu/state/*` + `/tofu/lock/*` (running code), **NOT** the survey-doc's `/tofu-state/*`. `/tofu/state/` IS the separate prefix lock 7 asks for and is already wired into `dr-backup.sh`. Renaming would orphan live state and break DR. **The live state-backend serves `/tofu/state/*` + `/tofu/lock/*`; these prefixes are FROZEN (do NOT rename).**
- State backend binds **overlay-only** on the control VM (`:8390` on the Nebula iface), reaching the founder etcd quorum via `/etc/mackesd/etcd-endpoints`. The current `0.0.0.0` bind becomes `STATE_BACKEND_BIND` (default the detected overlay IP).
- **Endpoint resolution (resolves GAP 4 + the live-infra correction):** `MCNF_ETCD` is NO LONGER defaulted to the dead `http://172.20.145.192:2379`. Every script (`tofu-state-etcd.py`, `state-backend-up.sh`, `mcnf-secret.sh`, `dr-*.sh`, `etcd-lib.sh`, `reconciler.env`) resolves endpoints in this order: (1) explicit `MCNF_ETCD` env, (2) `/etc/mackesd/etcd-endpoints` (comma-joined `http://<ip>:2379`), (3) FAIL LOUD. The live quorum is the LIGHTHOUSES (nyc3 10.42.0.4 / fra1 10.42.0.5 / sfo3 10.42.0.6 + Eagle 10.42.0.2). The LAN control node 172.20.145.192 (rocky9-kvm2) is a SEPARATE concept (the host that may run Phase A / be reconstituted) and is never an etcd endpoint.
- Bootstrap order is FIXED: (1) `found` + `setup-etcd.sh --init` on the founding lighthouse (writes its overlay endpoint); (2) control VM enrolls + `setup-etcd.sh --client-only --anchors <quorum>`; (3) `state-backend-up.sh` (overlay bind, endpoints from file); (4) per-mesh `backend.tf`/`backend.hcl` generated; (5) `tofu init`. Tofu never provisions its own state store.
- Backend config is GENERATED per-mesh from `infra/tofu/backend.tf.tmpl` (placeholders `__CONTROL_IP__` / `__ROOT__`) + a per-root `*.backend.hcl` passed via `-backend-config=`, never a hardcoded `.192`.

**Architecture:** EDIT `tofu-state-etcd.py` (multi-endpoint comma-split `MCNF_ETCD`, `STATE_BACKEND_BIND`, prefix verbatim, fail-loud if no endpoint); EDIT `state-backend-up.sh` (source endpoints from `/etc/mackesd/etcd-endpoints`, fail loud if absent — NO `.192` default); NEW `gen-backend-config.sh` + `backend.tf.tmpl`; NEW `state-backend-bootstrap.sh` (the ordered come-along hook with the Phase-A overlay precheck). Multi-endpoint failover is naive try-next (NOT linearizable reads — acceptable, Tofu re-locks before write).

### 2.3 secrets — secret-zero RESOLVED (on-VM keygen, no passphrase-in-state)
**Locks:**
- The mesh secret store (`age + etcd`, `mcnf-secret.sh`) is the single source of truth for all backoffice creds; no new engine.
- Credential set under `/mcnf/secret/*`: `do-token`, `xapi-password`, `dns-token`, `edgeos-cred`, `xo-token`, `forgejo-admin-pass`, `forgejo-runner-token`, `forgejo-secret-key`, `sccache-access-key`, `sccache-secret-key`, `dr-spaces-key`.
- **Secret-zero mechanism (resolves GAP 1, GAP 6, CONTRADICTION 1):** the control VM is NOT handed any pre-existing key. At first boot it runs `mcnf-secret.sh init-self`:
  1. generate a FRESH age keypair into `/root/.mcnf-age-key` (0600) — the private key NEVER leaves the VM, never appears in tofu state, never transits the network.
  2. publish its recipient (public key) to etcd `/mcnf/age-recipients/<node-id>` (non-secret; only a public key).
  3. (no plaintext anywhere — only a public recipient is written to the mesh).
  Then a **re-seal** step (`mcnf-secret.sh reseal-to <recipient>`, run by the operator or the etcd leader) decrypts each `/mcnf/secret/*` value with the EXISTING mesh key and RE-ENCRYPTS it multi-recipient (`age -r <mesh-recipient> -r <control-vm-recipient> …`) so the control VM's own key can `get` every cred. age natively supports repeated `-r`; today's `mcnf-secret.sh put` uses a single `-r` and is extended to read the full recipient set from `/mcnf/age-recipients/*`.
- **"The mesh age identity" is now precisely defined (resolves GAP 6):** it is the age X25519 private key at `/root/.mcnf-age-key` whose recipient is published at `/mcnf/age-recipient`. It is the key that decrypts `/mcnf/secret/*`. It is DISTINCT from the Nebula CA private key (`DEFAULT_CA_KEY_PATH`, the #12 payload). The two are never conflated. The control VM does NOT receive either master key; it mints its own and is granted read access by re-seal.
- **Ordering on a fresh VM (resolves GAP 6):** keygen+register (`init-self`) precedes any `mcnf-secret.sh get`; the re-seal (operator/leader) must have run before Phase 1 of `backoffice-up.sh` can unseal creds. `backoffice-up.sh` Phase 0 verifies the VM's recipient is present in the re-sealed envelopes (a self-test `get` of a sentinel key) and fails loud with the exact `reseal-to` command if not.
- **If instead extending the enroll bundle (documented alternative, NOT chosen):** it would require a NEW backoffice-scoped sealed field gated by a `is_backoffice_bearer` ledger note (mirroring `is_lighthouse_bearer` at `nebula_enroll.rs:750`), delivered to `--role server` enrollments, NEVER conflated with `ca_key_pem`. We reject this in favor of on-VM keygen because it adds crypto surface and still moves a master secret across the wire; on-VM keygen moves only a public key.
- Per-mesh config (mesh-id, project, regions, lighthouse endpoints) is GENERATED at found time into a non-secret etcd doc `/mcnf/backoffice/config` (+ `/mcnf/site/*`) and rendered into `*.auto.tfvars`; only tokens come from `/mcnf/secret/*`.
- Rotation = atomic etcd overwrite at the same key + provider-side revoke; consumers re-resolve on next `env.sh` source.

**Architecture:** EXTEND `mcnf-secret.sh` (`init-self` generate-key+register-recipient; `reseal-to <recipient>` / `reseal-all` multi-recipient re-encrypt every `/mcnf/secret/*` to the full `/mcnf/age-recipients/*` set; `rotate`; `put`/`get` read the recipient SET); NEW `mcnf-config.sh` (`gen`/`render`); fold `/root/.mcnf-xo-token` + `/root/.mcnf-ubnt-cred` into the store; `dr-backup.sh` carries `/mcnf/secret/*` + the recipient set, with a SEPARATE operator-run passphrase-sealed CA+identity bundle. (The `mackesd secret-seal/secret-unseal` thin CLI over `ca::backup::seal_bytes`/`unseal_bytes` is STILL built — but used ONLY for the separate operator-run DR key bundle, not for control-VM bootstrap.)

### 2.4 forgejo-ci (Full tier)
**Locks:**
- Forgejo + act_runner live ON the control VM, not the LAN dev host. sqlite DB stays; `SECRET_KEY`/admin-pass/runner-token are the only durable state — stored in `/mcnf/secret/forgejo-*`, never plaintext.
- GitHub remains canonical upstream; Forgejo PULL-MIRRORS when reachable, else seeds from the on-disk `/root/magic-mesh` clone (air-gap path).
- Runner is host-native (systemd, label `farm`) so `runs-on: farm` jobs inherit the mesh SSH key + overlay.
- CI builds RPMs via the existing `build-rpm-fedora43.sh` and stages UNSIGNED into a HOLD area of a sovereign Forgejo-served dnf channel (gh-pages-shaped). Signing stays operator-gated (`sign-release.sh`, the `/release` step) — never CI.

**Architecture:** generalize `forgejo-up.sh`/`forgejo-runner-up.sh` (control-VM overlay IP, secret-store-backed); NEW `forgejo-seed.sh`, `forgejo-deploy.sh`, `dnf-channel-up.sh`; NEW `.forgejo/workflows/rpm-publish.yml` (`runs-on: farm`, build → stage unsigned → createrepo into hold). `do-lighthouse-cloudinit.sh` already templates `REPO_BASEURL` → point at the sovereign channel.

### 2.5 reconciler (Full tier, lock 12)
**Locks:**
- Two distinct reconcilers stay distinct: the autoscale reconciler (`install-helpers/farm-reconciler.sh`, 5-min, gated tofu apply + build-ready provisioning) and the @farm build reconciler (`automation/reconciler/farm-reconcile.sh`, 15-min, converges @farm jobs into builds+PRs). NOT merged.
- Reconciler is systemd-managed on the control VM (continuous loop). The standing LIVE timer (FA_APPLY=1) is armed ONLY by explicit operator action (`enable-autoscale-timer.sh`), never by AI or genesis. Genesis lands units **plan-only**.
- Durable state (busy-state, dwell, last-good shapes) moves OFF host-local `/var/lib` INTO etcd `/reconciler/*`.
- Hardcoded LAN constants (`.192`, XO ws URL, dom0 hosts, the `.claude/worktrees` path, the dead `MCNF_ETCD` default) externalized to `/etc/mcnf/reconciler.env` generated at deploy time from mesh + tofu state; `MCNF_ETCD` rendered from `/etc/mackesd/etcd-endpoints`.
- The apply gate (FA_APPLY ∧ reachable ∧ state-sane ∧ golden-set + pre-mutation re-confirm) preserved verbatim. **Apply-reachability probe target changes from dead XO to the founding-dom0 XAPI :443** (see 2.8 resolution).
- Apply-time secrets pulled from the secret store, never to repo/journal.

**Architecture:** NEW `reconciler-up.sh` (renders `/etc/mcnf/reconciler.env`, installs both units `WorkingDirectory=/opt/mcnf`, enables plan-only); EDIT `enable-autoscale-timer.sh` (`${MCNF_REPO:-/opt/mcnf}`, the one arming step, and the golden-template name reconciled — see §2.8); NEW `reconciler-state.sh` (etcd `/reconciler/*` get/put/cas reusing the `mcnf-secret.sh` etcd HTTP v3 pattern); NEW `render-env.sh`; NEW oneshot `mcnf-reconciler-bootstrap.service` (ensures prefix). `MCNF_REPO` must point at a **dedicated release slot**, never the resettable `.52` build dir (CI gremlin).

### 2.6 dr (DR — Full tier)
**Locks:**
- Reuse `dr-backup.sh`/`dr-restore.sh` as the spine; add a **manifest v2** + three glue scripts (`dr-snapshot-onmesh.sh`, `dr-push-offfleet.sh`, `dr-reconstitute.sh`).
- On-mesh first line = copy every `dr-<ts>.age` into `/mnt/mesh-storage/dr/` (Syncthing-replicated Mesh-Sync), retained N-deep.
- Off-fleet PUSH (mcnf-dr-4533 DO Spaces) stays a SEPARATE operator-run script; the agent never executes it (spend/egress); Spaces key from `/mcnf/secret/dr-spaces-key`.
- Nebula CA + mesh age identity backed up via a SEPARATE operator-run passphrase-sealed bundle (this is the ONE place `mackesd secret-seal`/`ca::backup::seal_bytes` passphrase sealing is used), NOT folded into `dr-<ts>.age` (the key cannot live inside the thing it decrypts).
- All DR scripts env-parameterised (`MCNF_ETCD` resolved from the endpoints file, `MCNF_AGE_KEY`, `MCNF_MESHFS_DIR`, `MCNF_FORGEJO_DATA`, `MCNF_DR_BUCKET`, `MCNF_HOST_IP`). **`dr-env.sh` no longer defaults `MCNF_ETCD` to `.192:2379`** — it resolves from `/etc/mackesd/etcd-endpoints`, failing loud if absent (the other path defaults — meshfs dir, forgejo data, bucket — keep their current-LAN values).
- etcd captured two ways: selective key-range manifest (`/tofu/state` + `/mcnf/secret` + `/mcnf/age-recipients`) for portable restore, PLUS a full consistent etcd v3 snapshot (from the leader endpoint) for whole-store reconstitution.

**Architecture:** NEW `dr-env.sh`; EDIT `dr-backup.sh` (manifest v2: + Forgejo data tarball via sqlite quiesce, + etcd snapshot, `dr_backup_version=2`); NEW `dr-snapshot-onmesh.sh` (first line + retention + `INDEX.json`); NEW `dr-push-offfleet.sh` (operator-run, dry-run default); NEW `dr-ca-bundle.sh` (operator-run, separate passphrase-sealed bundle via `mackesd secret-seal`); NEW `dr-reconstitute.sh` (guided rebirth, `--verify` stops before mutation, post-restore asserts a specific repo + admin row — see §2.4 of the acceptance).

### 2.7 genesis-hook + bring-up ordering (the spine)
**Locks (consolidated — these resolve the multiple proposed orchestrators into ONE):**
- The bring-up is ONE ordered orchestrator `automation/backoffice/backoffice-up.sh` reading declarative tier manifests (`manifest.minimal.toml` / `manifest.full.toml`), sequencing the EXISTING up-scripts. Pure ordering + idempotency + readiness glue; no service logic reimplemented.
- `mackesd found` does NOT itself run the heavy bring-up: `--with-backoffice` records intent to `/mcnf/backoffice/intent` `{tier,host,ts}`, provisions the control VM (`control-vm-provision.sh`, live-gated), and the control VM runs `backoffice-up.sh`. cmd_found's local CA/overlay work is non-destructive; VM create + tofu apply + farm provision are operator-gated.
- `MCNF_CONTROL_IP` / `MCNF_BACKOFFICE_HOST` resolves to the control VM's overlay IP (discovered from the founding bundle / `mackesd peers --json`), de-hardcoding `172.20.145.192` everywhere. With `MCNF_CONTROL_IP=172.20.145.192` the scripts reproduce today's behavior byte-for-byte (the reconstitute arm). The dead `.192:2379` etcd default is removed everywhere (resolved); `MCNF_CONTROL_IP=.192` retargets the state-backend HOST, not the etcd endpoint.
- Idempotent re-convergence via a SETUP-7-style emitted facts file `automation/backoffice/.state/backoffice-site.yml` (tier + control-VM identity + endpoint IP + enabled phases); `--adopt` mode detects existing services instead of recreating (proven against the live `.192` node — container IDs untouched).
- Reconstituting the hand-built LAN setup is the SAME orchestrator pointed at the live founder etcd + a DR-restored manifest, NOT a separate code path.

**Architecture / bring-up order (encoded in manifests):**

```
PHASE 0  PRECHECK   overlay up + Phase-A runner is an enrolled overlay member + founder etcd
                    reachable via /etc/mackesd/etcd-endpoints + this VM's recipient is in
                    the re-sealed secret envelopes (sentinel get)            [resolves GAP 3,6]
PHASE 1  SECRETS    mcnf-secret.sh init-self (keygen+register) [done at boot] + get DO/XAPI/
                    Forgejo creds (resolved from the VM's own key)            [lock 8]
PHASE 2  STATE      state-backend-up.sh (overlay:8390, endpoints from file) + gen-backend-config.sh
                    + tofu init -migrate-state                                [locks 2,7]
PHASE 3  TOFU-ROOTS validate each root (xen-xapi, zone1-do, edgeos, control-vm) tofu plans against
                    the etcd backend
         ====================== MINIMAL TIER STOPS HERE (lock 4/9: can apply infra) ======================
PHASE 4  CI         forgejo-up.sh + forgejo-runner-up.sh + dnf-channel-up.sh (control-VM overlay IP) [lock 10]
PHASE 5  RECONCILER reconciler-up.sh: enable both timers PLAN-ONLY (NOT armed)  [lock 12]
PHASE 6  BUILD-FARM tofu apply xen-xapi (LIVE-GATED) + bake/clone golden + toolchain + sccache [lock 11]
PHASE 7  DR         enable dr-snapshot-onmesh timer; off-fleet push stays operator-run
FINAL    emit backoffice-site.yml (idempotency ledger)
```

### 2.8 tofu-reconstitution + build-farm-provision (resolved together)
**Resolved contradiction (newest/most-specific wins):** The build farm provisioning root is **`infra/tofu/xen-xapi/`** (XAPI-native, no XO). The top-level XO-based `infra/tofu/main.tf` + `build-vms.tf` (xenorchestra) path is **deprecated** — XO is dead live (ws connection-refused), so any reconciler applying it degrades to plan-only forever.

**The `for_each` port WITH migration (resolves CONTRADICTION 2 + STUB 1):** Today `xen-xapi/build-vms.tf` is THREE hardcoded `resource "xenserver_vm" "build_50/51/52"` blocks with `ignore_changes`. Porting these to `for_each` over `local.build_vm_specs` CHANGES their resource addresses from `xenserver_vm.build_50` to `xenserver_vm.build["xen-bigboy"]` (etc.), which OpenTofu reads as destroy-old + create-new of the LIVE build VMs. To prevent that, the port ships with **`moved {}` blocks** in `build-vms.tf` mapping each old address to its new `for_each` key:
```hcl
moved { from = xenserver_vm.build_50  to = xenserver_vm.build["xhs-50"] }
moved { from = xenserver_vm.build_51  to = xenserver_vm.build["kvm-51"] }
moved { from = xenserver_vm.build_52  to = xenserver_vm.build["big-52"] }
```
The acceptance is therefore NOT "shape={} → 0-add" by assertion; it is **a real `tofu plan` against the relocated etcd backend that shows 0-add / 0-change / 0-destroy** with the `moved{}` blocks in place (and FAILS the task if any destroy appears). `moved{}` is preferred over manual `tofu state mv` because it is declarative + reproducible on any operator's checkout.

**Golden-template name canonicalization (resolves GAP 5):** the toolchain template builder (`setup-xcp-golden-template.sh`) and the adopt-only resources both use **`MDE-VM-golden`**, while `infra/tofu/variables.tf` default and `enable-autoscale-timer.sh` reference `MDE-VM-golden-tc`. **CANONICAL NAME = `MDE-VM-golden`** (the name the actual template-builder produces and the live VMs clone). DAR-34 bakes the toolchain INTO `MDE-VM-golden` (no separate `-tc` artifact). Migration: `infra/tofu/variables.tf` default and `enable-autoscale-timer.sh` are edited from `MDE-VM-golden-tc` → `MDE-VM-golden`; a one-time check (`grep -r 'MDE-VM-golden-tc'`) must return only historical-doc mentions; if a `MDE-VM-golden-tc` template object exists live it is renamed/retired to `MDE-VM-golden` (live-gated step in DAR-34). The "baked toolchain" is conveyed by the template CONTENT, not a name suffix.

**edgeos state → etcd (resolves GAP 2 + CONTRADICTION 3):** `infra/tofu/edgeos/` currently uses LOCAL state (no `backend.tf`, `terraform.tfstate` on disk) and the deprecated top-level `infra/tofu/main.tf` is also local. For DR to capture it and the generator to apply it, edgeos is migrated to the http/etcd backend at `/tofu/state/edgeos`: `gen-backend-config.sh` now writes `infra/tofu/edgeos/backend.tf` too, and a new task (`DAR-9b`) does the one-time `tofu state push`/`-migrate-state` of the on-disk edgeos state into etcd with a 0-add/0-destroy parity gate. The "four roots uniform" acceptance is only TRUE once edgeos has a backend AND `xen-xapi`/`zone1-do` no longer hardcode `.192`.

**The literal-`.192`-in-tracked-`.tf` contradiction (resolves CONTRADICTION 3):** `xen-xapi/backend.tf` and `zone1-do/backend.tf` contain the literal `http://172.20.145.192:8390`. OpenTofu backend blocks cannot interpolate variables, so the literal is REMOVED from the tracked `.tf` (the block keeps only `lock_method`/`unlock_method`) and the address is supplied via `-backend-config=<root>.backend.hcl` (generated, gitignored). The grep gate (`grep 172.20.145.192 infra/tofu/*/backend.tf`) must return EMPTY for the task to be done.

**Locks:**
- Per-mesh tfvars GENERATED from mesh identity (founding bundle + `/mcnf/site/*`), never hand-edited; no hardcoded `.192`/`matthewmackes.com`/pool UUIDs in root `.tf`.
- State backend address is a per-mesh `*.backend.hcl` passed via `-backend-config=` (Tofu backend blocks can't interpolate vars — the literal must be REMOVED, not shadowed).
- Provider creds unsealed at apply time from the secret store into process-scoped env, never a persisted file.
- xen-xapi/build-vms learn a new dom0 from a declarative `dom0_registry` map discovered by a one-shot XAPI probe (`learn-dom0.sh`); provider aliases RENDERED from a `providers.tf.tmpl` (HCL can't `for_each` a provider block).
- edgeos is gateway-OPTIONAL + site-parameterized (`var.enabled=false` → Minimal never applies it) AND now etcd-backed.
- Live migration is STATE-MOVE only (`tofu state pull/push` + `moved{}` + re-point backend), never destroy/recreate; import-parity (0-add/0-destroy plan) is the safety gate.
- `MDE-VM-golden` carries the BAKED toolchain (rust 1.94 + dev libs + mold + sccache); `provision_build_ready` collapses to taking the clean baseline snapshot; per-clone ansible stays as the bare-template fallback.
- sccache S3 (minio) is a farm-provisioned mesh-resident service; endpoint from a tofu var/output, creds from `/mcnf/secret/sccache-*`, never the hardcoded `:9000` on `.192`.
- The drain/slot model is the EXISTING per-node flock in `farm-dispatch.sh` + the `@farm-job` binding + inter-job snapshot-revert (`farm-vm-snapshot.sh reset`). No new scheduler.

**Architecture:** NEW `automation/tofu-reconstitute/{gen-tfvars.sh,tofu-env.sh,learn-dom0.sh,reconstitute.sh,migrate-state.sh}`; NEW `infra/tofu/control-vm/`; NEW `infra/tofu/backend.tf.tmpl` + per-root `*.backend.hcl` seam; EDIT `infra/tofu/{xen-xapi,zone1-do}/backend.tf` (strip the literal address); NEW `infra/tofu/edgeos/backend.tf` (generated); EDIT `xen-xapi/build-vms.tf` (`for_each` + `moved{}`); NEW `automation/farm/{farm-adopt.sh,sccache-backend-up.sh,farm-bootstrap.sh}`; NEW `install-helpers/bake-build-golden.sh`; EDIT `infra/tofu/variables.tf` + `enable-autoscale-timer.sh` (golden name → `MDE-VM-golden`); EDIT `sccache.yml` (required `-e` endpoint, no `.192` default).

---

## 3. Tier Model

| Component | Minimal | Full |
|---|:---:|:---:|
| Mesh etcd reachable (founder quorum via `/etc/mackesd/etcd-endpoints`) | ✅ (precondition) | ✅ |
| Secret store unsealed (control VM mints own key + re-seal grants read) | ✅ | ✅ |
| State backend (`tofu-state-etcd.py`, overlay:8390) | ✅ | ✅ |
| Four Tofu roots wired incl. edgeos@etcd (can `apply` infra) | ✅ | ✅ |
| Control VM as full mesh peer (`--role server`) | ✅ | ✅ |
| Forgejo + host-native runner + sovereign dnf channel | — | ✅ |
| Reconciler + autoscaler timers (plan-only at genesis) | — | ✅ |
| Build farm provisioned (xen-xapi `for_each` shape model + `moved{}` + sccache + golden) | — | ✅ |
| DR (on-mesh first line + manifest v2 + scheduler) | — | ✅ |

One VM image, one tofu root, one orchestrator. The difference is the manifest the orchestrator reads and which already-existing scripts get a systemd unit enabled.

---

## 4. End-to-End Bring-Up Process

1. **Operator prefill (before genesis):** `mcnf-secret.sh put` the required creds (DO token, XAPI password(s), Forgejo admin pass, and for Full: sccache keys, dr-spaces-key). The runbook checklist enumerates ALL of them up front.
2. **Genesis:** `mackesd found <mesh> --with-backoffice[=minimal|full]`. found mints the CA, founds the overlay, ensures the founding dom0/Phase-A runner is an overlay member, runs `setup-etcd.sh --init` on the lighthouse, emits SETUP-7 facts, records `/mcnf/backoffice/intent`, and (live-gated) invokes `control-vm-provision.sh`.
3. **Control VM:** `infra/tofu/control-vm/` applies on the founding dom0 (Phase A backend = founder etcd, endpoints from file). The VM boots, renders the static-IP NM keyfile, runs `mackesd join --role server`, **mints its OWN age key + registers its recipient**, runs `mackesd converge`, runs `setup-etcd.sh --client-only`. The operator/leader runs `mcnf-secret.sh reseal-to <vm-recipient>` so the VM can read the store.
4. **Backoffice-up:** the control VM runs `backoffice-up.sh --tier <t>` → Phases 0–3 (Minimal) or 0–7 (Full). State-backend RELOCATES onto the VM; backends repoint to the VM overlay IP.
5. **Arm (operator, Full):** `enable-autoscale-timer.sh` flips FA_APPLY=1 when XAPI + state + golden are healthy.
6. **Reconstitute (existing LAN):** run the SAME `backoffice-up.sh` with `MCNF_CONTROL_IP=172.20.145.192 --adopt`, or cold-restore via `dr-restore.sh --prod` then `backoffice-up.sh`.

---

## 5. Acceptance (epic-level, runtime-observable)

- A new mesh founded with `--with-backoffice=minimal` ends with: state-backend serving `/state/<root>` on the control VM overlay IP, secret `get` working **using the VM's own key** (the VM never received a master key), all four roots (incl. edgeos@etcd) `tofu plan` clean against the etcd backend — with **no hardcoded `172.20.145.192`** in any live default (`grep` clean across `automation/` + `infra/tofu/*/backend.tf`) and **no `.192:2379` etcd default** anywhere.
- A **`grep` of the produced `terraform.tfstate` for each root shows NO plaintext age private key / no unseal passphrase / no provider token** (resolves STUB 1) — the secret-zero design is verified by inspecting state, not asserted.
- `--with-backoffice=full` additionally: Forgejo `/api/healthz=pass`, runner `farm` online, both reconciler timers `is-active` (plan-only), a `@farm` job drives a real build VM to a `pass` (live-verify, observed on the real VM — resolves STUB 2), DR first-line artifact in `/mnt/mesh-storage/dr/`.
- The live `.192` setup reconstitutes via the same orchestrator with **0-destroy** plans on all roots (the `xen-xapi` `for_each`+`moved{}` plan is 0/0/0 against live VMs); the `/root/.mcnf-*` plaintext fallback files can be removed without breaking `tofu plan`.
- DR round-trip: backup → restore into a throwaway etcd → `tofu plan` matches AND the restored Forgejo serves `/api/healthz=pass` with a **named seed repo present and an admin row in `user`** (resolves STUB 3); the CA/identity bundle stays SEPARATE and passphrase-sealed.

---

## 6. Risks (carried from subsystem designs)

- **xenserver provider 0.2.x create-with-cloud_config is less proven than the adopt path.** Validate the seed actually attaches on a throwaway clone before relying on it (CONTROLVM-9 / FARMPROV-1).
- **Chicken-and-egg repoint must be atomic + DR-snapshotted first**, or state splits between founder-etcd and VM-etcd.
- **The `for_each` port is destroy-prone:** without the `moved{}` blocks the very first plan would propose destroying the live build VMs. The `moved{}` blocks + a 0-destroy parity gate are load-bearing.
- **Overlay etcd is plain-HTTP, unauthenticated** — security rests entirely on Nebula being the only path; the overlay-only bind is load-bearing and must be asserted at runtime, not assumed. (Same for the re-seal: only PUBLIC recipients transit the mesh.)
- **Re-seal grants standing read access** to the control VM's key for all `/mcnf/secret/*`; a compromised control VM can read every backoffice cred. This is inherent to the role (the VM runs CI/reconciler/DR) and is the same trust the old `.192` node held; rotation (`mcnf-secret.sh rotate` + recipient drop) is the containment lever.
- **Cold-fact mirrors** (dom0 arrays in `farm-reconciler.sh` + ported `local.dom0`) can drift; reconcile or a genuinely different dom0 layout needs the registry regenerated.
- **Build-dir CI gremlin** (`.52` resets to `b6d4ca0`): the control VM repo checkout MUST be a dedicated release slot or the reconciler runs stale code.
- **Single founding dom0 is a SPOF** for the control VM host; HA is out of scope (follow-up).
- **Off-fleet bucket keys are console-minted** (MEDIA-2 blocker class): DR-4/DR-5 stay dry-run until the operator seals `dr-spaces-key`.
- **DC-18 genesis wizard lands in parallel** — the `--with-backoffice` flag + Backoffice wizard step + new `action/dc/backoffice-plan` verb must be additive to the moving `cmd_found`/`genesis.rs`/`datacenter.rs` action allow-list, not collide with the Step enum / action allow-list.
- **Live-gated units stay operator-gated even with FA_APPLY lifted** (real XCP-NG apply, real enroll, off-fleet DR push).

---

## 7. Out of Scope

Control-VM HA (ha-shadow-master follow-up); etcd clustering beyond the current quorum; multi-region farms; signed-value MAC on secret writes; auto-execution of the off-fleet DR/CA push (stays operator-run); the bundle-extension secret-zero alternative (documented but rejected in favor of on-VM keygen).