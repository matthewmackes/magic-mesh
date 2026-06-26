# Recon: "3 new Lighthouses up, retire the old 2, move Eagle onto the new set"

> **Status:** RECON ONLY — captured 2026-06-26 from a 5-agent sweep (920s) so a
> context clear does not force a re-run. No execution has happened. Read this
> before touching lighthouses/Eagle, then live-reconcile §0 before acting.
>
> Companion memory: `lighthouse-eagle-migration-recon` (load-bearing facts + pointer here).
> Related: `docs/ops/do-lighthouses.md`, `docs/design/zones.md`, `infra/tofu/zone1-do/`,
> memory `lighthouse-access-durable`, `live-verify-deploys`.

---

## 0. LIVE-VERIFIED ground truth (2026-06-26, operator-approved SSH reconcile)

The §0 "disagreement" below was resolved by SSHing both anchors + `doctl` (context
`mackes`). **Reality, confirmed:**

- **Exactly ONE true Nebula lighthouse:** `lh1` = host `lh-4533-1782213858`, overlay
  `10.42.0.1` / public `167.71.247.150`, **DO droplet `579695013`**, tag `magic-lighthouse`,
  `am_lighthouse: true`. Bundle roster lists ONLY itself. mesh_id `4533`, CIDR
  `10.42.0.0/16`, epoch 0. CA lives here.
- **`mcnf-lh2` is NOT a lighthouse** — overlay `10.42.0.3` / `68.183.55.253`, **droplet
  `579847089`**, tag `mcnf-lighthouse`, but `am_lighthouse: false`; only dials `10.42.0.1`.
  A peer named like a lighthouse.
- **Eagle** = `UNIT-EAGLE`, overlay **`10.42.0.2`** (confirmed; NOT `.4`), LAN
  `172.20.146.13` — peer + etcd member. ⚠ Both `id_ed25519` and `mackes_mesh_ed25519`
  are **rejected** on Eagle SSH — need the right key from the operator.
- **etcd quorum = 3 healthy members:** lh1 `10.42.0.1`, Eagle `10.42.0.2`, mcnf-lh2
  `10.42.0.3` (consistent from both anchors).
- **DO droplets (`doctl --context mackes`):** `579695013` lh-4533… / `579847089`
  mcnf-lh2 / `440041476` `ASTERISK-1-MACKES` (VoIP — **do NOT touch**). The 2 to delete =
  the first two.
- **Tofu `zone1-do` is FULLY STALE** — its `lighthouse-01` (id `579112110` / `174.138.68.216`)
  no longer exists in DO. **"Delete the old 2" = `doctl compute droplet delete`, NOT
  `tofu destroy`.**
- **Multi-true-lighthouse has never worked here** — every distributed bundle carries only
  `10.42.0.1` (confirms the `nebula_csr_watcher` single-`10.42.0.1` hardcode). Ending at
  **3 _true_ lighthouses** (all `am_lighthouse:true`, all dialed) is partly a CODE task.

The original recon §0–§5 below is retained for the file/command map; treat the IP/role
claims through the lens of this verified block.

---

## 0b. The (now-resolved) disagreement — kept for context

Two sources disagreed about what lighthouses are live, and resolving this decided
whether "delete the old 2" is `tofu destroy` or `doctl droplet delete`:

| Source | Says the lighthouses are… |
|---|---|
| **WORKLIST live-state** (`docs/WORKLIST.md:909-921`, 2026-06-23/24) — treat as **ground truth** | **lh1** `167.71.247.150` / overlay `10.42.0.1` (founding anchor); **lh2** `mcnf-lh2` `68.183.55.253` / overlay `10.42.0.3`. Plus **Eagle** `10.42.0.2` = the 3-member etcd quorum. |
| **Tofu `zone1-do`** (`infra/tofu/zone1-do/main.tf`) — **DRIFTED** | one managed droplet `lighthouse-01` `174.138.68.216` / overlay `10.132.0.3` (nyc3, droplet id `579112110`). DNS records `lighthouse-02` `45.55.33.179` and `lighthouse-03` `138.197.32.202` are explicitly **STALE — no droplets**. |

The live anchors (`167.71.x`, `68.183.x`) are **not in Tofu state at all** — they
were almost certainly stood up imperatively (`install-helpers/do-lighthouse-up.sh`)
or hand-rolled, outside Tofu. The local `zone1-do/terraform.tfstate` is **0 bytes**;
real state lives in the remote backend (`backend.tf` → `http://172.20.145.192:8390/state/zone1-do`).

**Phase 0 (do before any change):** SSH the live anchors and Eagle, capture truth:
- `root@167.71.247.150`, `root@68.183.55.253` (via `id_ed25519` — see memory `lighthouse-access-durable`)
- Eagle `172.20.146.13` (overlay `10.42.0.2`)
- On each: `/etc/nebula/config.yaml` (the live `static_host_map` + `lighthouse.hosts`),
  the live `roster_from_directory` output, the **etcd member list**, and whether each
  droplet exists in the DO account / Tofu state (`doctl compute droplet list`).

---

## 1. Current state

**Live anchors (authoritative):**
- **lh1** — public `167.71.247.150`, overlay `10.42.0.1` (founding anchor; mints the CA)
- **lh2** — `mcnf-lh2`, public `68.183.55.253`, overlay `10.42.0.3` (joined role=lighthouse)
- **Eagle** — `UNIT-EAGLE` / ".13", LAN `172.20.146.13`, overlay `10.42.0.2`. **NOT a
  lighthouse** — a peer + **1-of-3 etcd quorum member** (`install-helpers/phase-b-retire-lizardfs.sh:51`).

**What "Eagle" is** (`docs/design/zones.md:12-19`): the operator-review **production
LAN workstation**, the middle stage of the release pipeline **Build (Xen) → Eagle →
DO (lighthouse roll)**. F44 box behind NAT. (Beware two decoys: `digitalocean_ssh_key.eagle`
id `57026397` in `main.tf:8,26-30`, and the "eagle" promotion-*stage* label in
`dc_promote.rs` — neither is the node.) Older 4-node mesh had Eagle at `10.42.0.4`;
current SUBSTRATE-V2 fleet has it at `10.42.0.2` — **confirm live before acting**.

**How the lighthouse set is controlled — there is NO `count`/`for_each`/list:**
- Lighthouses are **individually hand-written `digitalocean_droplet` resources** in
  `infra/tofu/zone1-do/main.tf`. You grow the set by **adding resource blocks**, not
  bumping a number. (The `count`/`for_each`/`var.shape`/`var.small_count` machinery in
  `infra/tofu/main.tf` is the **Xen build-farm autoscaler** — a different fleet.)
- Shared **shape** knobs (not count) — `infra/tofu/zone1-do/variables.tf`:
  `lighthouse_size` (`s-2vcpu-2gb`), `lighthouse_image` (`fedora-43-x64`),
  `region` (`nyc3`), `domain` (`matthewmackes.com`).
- **Two grow paths:**
  1. **Manual** — uncomment the `digitalocean_droplet.lighthouse_04` template at
     `main.tf:110-117` (README says rename to `-02`), `tofu apply`.
  2. **Programmatic (DATACENTER-19)** — `action/dc/lighthouse-create` RPC
     (`crates/mesh/mackesd/src/ipc/datacenter.rs`, `lighthouse_create_resource`)
     appends a droplet block to the GENERATED `infra/tofu/zone1-do/dc-lighthouses.tf`
     (**not on disk yet — DC-19 has never run to completion live**). Idempotent;
     duplicate name rejected. Tags `magic-lighthouse`, registers
     `digitalocean_ssh_key.mackes_mesh_claude.id`.
- Third path: imperative `install-helpers/do-lighthouse-up.sh` + `do-lighthouse-cloudinit.sh`
  founds a **throwaway isolated mesh** via doctl, **outside Tofu state**.

**How Eagle points at lighthouses — NO repo file to edit:**
- Eagle is a **peer**: it dials lighthouses; they don't dial it.
- Its `/etc/nebula/config.yaml` is rendered **at runtime on the node** by `mackesd`'s
  `nebula_supervisor` (`crates/mesh/mackesd/src/workers/nebula_supervisor.rs`,
  `render_config_yaml_inner` ~432-532; written by `materialize_config` ~316-369),
  which emits `static_host_map` (`"<overlay_ip>": ["<public ip:4242>"]`) and
  `lighthouse.hosts` (overlay-IP list) **from `bundle.lighthouses`**.
- The roster comes from the live replicated peer directory via
  `mackes_mesh_types::lighthouse::roster_from_directory`
  (`crates/mesh/mackes-mesh-types/src/lighthouse.rs` ~203-256) — **every
  `role==lighthouse` record carrying BOTH `overlay_ip` AND `external_addr`**. So Eagle
  learns ALL lighthouses; losing one leaves the rest dialable. (`MESH-2` guard: always
  uses the public `external_addr`, never a local interface IP.)
- **The cutover is an operational act on the node, not a git commit.**

**Nebula config is NOT ansible/Jinja-templated** — it is rendered in-process by
`mackesd`. The only YAML "template" is `site.yml` (`crates/mesh/mackesd/src/site_yml.rs`
+ `ansible/roles/mackes/`), the **systemd-service convergence playbook**, which carries
an `mde_lighthouses` fact but does **not** generate `static_host_map`.

**CA / cert flow** (bringing up a new lighthouse): all ops shell out to `nebula-cert`
via `NebulaCertBackend`/`SubprocessBackend` (`crates/mesh/mackesd/src/ca/mod.rs`). CA at
`/var/lib/mackesd/nebula-ca/ca.{key,crt}`. `mackesd found` / `mackesd mesh-init`
(`mesh_init.rs`, `bin/mackesd.rs` `cmd_found` ~8362): pin role → `mint_ca` (idempotent)
→ self-sign host cert (`ca/sign.rs`, overlay IP from `10.42.0.0/16` /17) → write own
`NebulaBundle` listing itself as `lighthouses[0]` with `external_addr` = `<public-ip>:4242`
→ mint join bearer. Joining peers' CSRs are auto-signed by the lighthouse-side
`nebula_csr_watcher` worker or the HTTPS `/enroll` listener. Cap `MAX_PEER_CAP=12`.

---

## 2. The change, in a safe order (never orphan Eagle)

**Governing rule:** stand up + enroll all 3 new lighthouses, confirm Eagle re-pointed
and quorum holds, **THEN** retire the old 2. Never drop below `HA_MIN_LIGHTHOUSES=2`
(`lighthouse.rs:196`) reachable, and never break the 3-member etcd quorum mid-flight.

**Phase 0 — Reconcile reality (BLOCKING).** See §0.

**Phase A — Stand up 3 new lighthouses (additive, reversible):**
1. Pick 3 regions (Open Q3). Use DC-19 region picker (`action/dc/do-regions` →
   `recommend_spread_region`) or set directly.
2. Create 3 `digitalocean_droplet` blocks (DC-19 RPC → `dc-lighthouses.tf`, or by hand
   in `main.tf`) + 3 matching `digitalocean_record.lighthouse_NN` A records.
3. **Operator-gated** `tofu apply` of `zone1-do` (outward-facing DO spend).
4. Bootstrap each: `mackesd found --role lighthouse` on the first new one (or join the
   existing mesh as role=lighthouse on the others); set `external-addr` to
   `<public-ip>:4242` (`lighthouse_addr.rs` → `/etc/mackesd/external-addr`). Each MUST
   publish a `role==lighthouse` directory record with **both** `overlay_ip` +
   `external_addr` or `roster_from_directory` won't advertise it.
5. **Verify the new 3 are in the live roster AND overlay-reachable before touching
   anything else.** You now have 5 lighthouses (old 2 + new 3) — over-provisioned but safe.

**Phase B — Cut Eagle over:**
1. Eagle picks up the new lighthouses automatically once they're in the replicated
   directory (its `nebula_supervisor` watches the bundle mtime, re-renders). Confirm
   re-render → `systemctl reload nebula.service` on Eagle.
2. Verify Eagle's new `/etc/nebula/config.yaml` `static_host_map` + `lighthouse.hosts`
   list the 3 new overlay IPs and that overlay handshakes succeed.
3. **Re-seat etcd quorum** onto the new lighthouses without ever losing quorum
   (add new members → confirm health → remove old). This touches the production
   coordination plane.

**Phase C — Retire the old 2 (destructive, LAST):**
1. Only after Eagle confirmed dialing the new set + quorum healthy on the new members.
2. `mackesd leave yes` on each old lighthouse (`crates/mesh/mackesd/src/leave.rs`):
   evicts its own cert fingerprint into the replicated `ca/blocklist`, removes roster
   files, wipes `/etc/nebula` + `role.toml`. (Irreversible re-add without re-enroll.)
3. Remove its directory record so it drops from `roster_from_directory`; confirm Eagle
   re-renders WITHOUT the old entries.
4. Delete droplets: **Tofu-managed** → remove blocks + operator-gated `tofu destroy`
   (⚠ whole-workspace only, see §3); **imperative** → `doctl compute droplet delete`.
   Prune the stale/old DNS A records in `main.tf` deliberately.

---

## 3. Precise files / commands per step

| Step | Edit / Run | Path |
|---|---|---|
| Reconcile | SSH + read live config + etcd member list | `root@167.71.247.150`, `root@68.183.55.253`, `172.20.146.13`; `/etc/nebula/config.yaml` |
| New droplet shape | (no edit — defaults fine) | `infra/tofu/zone1-do/variables.tf` |
| New droplets (manual) | add 3 `digitalocean_droplet.lighthouse_NN` (model on `main.tf:79-90` / template `:110-117`) + 3 `digitalocean_record.lighthouse_NN` | `infra/tofu/zone1-do/main.tf` |
| New droplets (DC-19) | `action/dc/do-regions` then `action/dc/lighthouse-create` (Workbench Network tab → `new_lighthouse_view` ~`panels/datacenter.rs:5631`) → writes blocks | `crates/mesh/mackesd/src/ipc/datacenter.rs`; generated `infra/tofu/zone1-do/dc-lighthouses.tf` |
| Apply (GATED) | `action/dc/tofu-apply {workspace:"zone1-do", confirm:true}` — also needs prod-arm | `crates/mesh/mackesd/src/ipc/tofu.rs:74-91`; gate `panels/datacenter.rs:672 prod_arm_allows` + `dc-prod-arm.json` |
| Bootstrap new LHs | `mackesd found --role lighthouse` / join role=lighthouse; set external-addr `<ip>:4242` | `bin/mackesd.rs` `cmd_found` ~8362; `lighthouse_addr.rs` |
| Eagle re-render | confirm `nebula_supervisor` re-render + `systemctl reload nebula.service` on Eagle | `workers/nebula_supervisor.rs` (`materialize_config`/`render_config_yaml_inner`) |
| Retire old LHs | `mackesd leave yes` on each old lighthouse | `crates/mesh/mackesd/src/leave.rs` |
| Destroy droplets (GATED) | `action/dc/tofu-destroy {workspace:"zone1-do",confirm:true}` (whole-workspace) OR `doctl compute droplet delete` | `tofu.rs`; `docs/ops/do-lighthouses.md` |
| Prune DNS | delete stale `lighthouse_02`/`_03` + old A records | `infra/tofu/zone1-do/main.tf:44-56` |
| Docs reflect | update host table | `docs/design/zones.md` |

---

## 4. Risks & operator gates

**Destructive / outward-facing (real money + irreversible):**
- `tofu apply`/`tofu destroy` create/delete real DO droplets ($$, DO API).
- ⚠ **`tofu-destroy` on `zone1-do` is whole-workspace only — no per-resource targeting**
  (`tofu.rs`). A blind destroy would also tear down `digitalocean_droplet.asterisk`
  (the VoIP box) and ALL DNS. **Do NOT run a blind workspace destroy** — surgically
  delete just the old lighthouses (`doctl`), or `tofu state rm` + targeted handling.
- `mackesd leave yes` blocklists the node's own cert fingerprint mesh-wide.

**Operator gates (all must hold — none auto-fire):**
1. `tofu-apply`/`tofu-destroy` refuse unless `confirm:true` (`ipc/tofu.rs:87 is_confirmed`,
   fails closed). Workspace allow-list = `xen-xapi | zone1-do | edgeos` only.
2. A `zone1-do` mutating op is ALSO refused unless the **prod-arm master switch** is
   armed (`panels/datacenter.rs:672 prod_arm_allows`; persisted
   `$XDG_CONFIG_HOME/mde/dc-prod-arm.json`; missing/corrupt reads as disarmed).
3. DC-19 `lighthouse-create` ONLY writes HCL — live apply + `mackesd found` + DNS add
   are carried, NOT auto-chained.
4. Datacenter tiles are navigate-only; destructive ops never fire from a tile click.

**What could orphan Eagle / wedge the mesh:**
- Deleting old lighthouses BEFORE the new ones are in `roster_from_directory` and Eagle
  has re-rendered → Eagle has no reachable lighthouse, falls off the overlay.
- Below `HA_MIN_LIGHTHOUSES=2` reachable → HA-degraded; below 1 → mesh down.
- Breaking etcd quorum (Eagle is 1-of-3 with old lh1+lh2) → coordination plane wedges.
- **INCIDENT-WEDGE precedent:** destroying old DO lighthouses once wedged the fleet by
  removing the LizardFS master; recovery was `unwedge-lizardfs.sh`. Keep it handy.
- `roster_from_directory` only advertises lighthouses with BOTH `overlay_ip` AND
  `external_addr` — a half-bootstrapped new LH (missing `external-addr`) is silently
  skipped, leaving Eagle short. Verify each new record is complete before retiring old.
- Architectural caveat: `do-lighthouses.md` framed multi-lighthouse-per-mesh as
  "out-of-scope roster work" yet the live mesh already runs 2; and
  `nebula_csr_watcher.rs:198-206` hardcodes a NEW peer's first bundle to a single
  `10.42.0.1` lighthouse — peers learn the full set only via the directory-roster
  reconcile, not at first enroll. Confirm the multi-LH join path before adding 3 more.

**Live-verify after EVERY phase** — green unit tests do NOT prove the overlay
re-pointed (memory `live-verify-deploys`). SSH the node, check the rendered
`/etc/nebula/config.yaml`, the live handshake, and etcd membership.

---

## 5. Open questions (answer before execution)

1. **(BLOCKING) Reconcile the IP disagreement** — see §0. Which lighthouses are real
   right now, and are they Tofu-managed or imperative? Decides `tofu destroy` vs `doctl delete`.
2. **Which 2 are "the existing 2" to delete?** The live lh1+lh2 (`167.71`/`68.183`), or
   the Tofu-recorded `lighthouse-01` + a stale DNS record? Not the same set.
3. **Regions for the 3 new lighthouses?** Default is all-`nyc3` (no geo spread =
   correlated-failure risk). Want a geo spread (e.g. nyc3 / sfo3 / fra1)?
4. **Final count = 3?** Request implies net 3 (2→5→3). Confirm — keep none of the old 2?
5. **Naming?** Template is `lighthouse_04`; README says rename to `-02`; DNS already
   advertises stale `-02`/`-03`. What names for the 3 new ones?
6. **Revoke old certs/IPs?** Blocklist retired LH certs (`mackesd leave yes`)? Prune the
   stale DNS records (`45.55.33.179`, `138.197.32.202`) + old A records, or leave them?
7. **etcd quorum reseat** — move quorum onto the new lighthouses, or keep Eagle +
   (which new pair)? Sequence so quorum is never lost.
8. **Execution surface** — live nodes only (re-enroll/re-render/restart), or ALSO reflect
   in Tofu DNS + `docs/design/zones.md`, or both?
