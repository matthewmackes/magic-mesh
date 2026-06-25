# SUBSTRATE-V2 fleet cutover runbook (LizardFS → etcd + Syncthing)

> **HISTORICAL — the cutover is COMPLETE.** LizardFS has been fully removed
> (SUBSTRATE-6, the LizardFS rip-out): the live fleet runs **etcd** (coordination)
> + **Syncthing** (files, plain `/mnt/mesh-storage` dir). The Phase-A/Phase-B
> staging, the `/etc/mackesd/etcd-endpoints` fs-path fallback, and the
> re-mount-LizardFS rollback below all describe the one-time transition and no
> longer apply — there is no LizardFS mount to fall back to or roll back onto.
> Kept here as the cutover record.

**Status: COMPLETE — LizardFS retired fleet-wide (SUBSTRATE-6).** Rehearsed on the
VM bed 2026-06-23, then rolled to the live fleet. This was the operator-gated
big-bang; it is kept as the historical runbook.

The substrate splits into two planes that cut over **independently**:

| Plane | From | To | Phase |
|---|---|---|---|
| Coordination (leader / peer directory / health) | `.mackesd-leader.lock` + peer JSON on the QNM-Shared FUSE mount | **etcd** (overlay-bound, lease-based) | **A** |
| Files (`/mnt/mesh-storage`) | **LizardFS** FUSE mount | **Syncthing** full-mesh on a plain dir | **B** |

Do **Phase A first, fleet-wide, in one flip**. Phase B (the LizardFS retirement, SUBSTRATE-6) is a separate later step. Both fall back automatically — mackesd reads `/etc/mackesd/etcd-endpoints`; absent ⇒ it uses the legacy fs path.

---

## 0. Pre-flight (must all be true)

- [ ] Every fleet node is on the release that carries the SUBSTRATE-V2 code **and** the FOUND-NEBULA-4/5 + reconciler + heartbeat fixes (this branch). Confirm: `rpm -q magic-mesh` matches the cut version on every node.
- [ ] The overlay is healthy fleet-wide: every node has a `nebula1` `10.42.x` address and can ping the founding lighthouse's overlay IP. (etcd + Syncthing bind to the **overlay**, so this is a hard prerequisite — validated: peer reachability works once the Nebula handshake completes.)
- [ ] A **rollback RPM** of the current production version exists and is reachable (the one-release back-out).
- [ ] You have the **overlay IPs** of: the founding anchor lighthouse, every other server/lighthouse, and every workstation.
- [ ] The bed rehearsal evidence has been reviewed (below).

Helper paths on every node: `/usr/libexec/mackesd/{cutover-substrate-v2,setup-etcd,setup-syncthing}`.

---

## Phase A — coordination onto etcd (stage everywhere, then flip together)

> **Why stage + flip, never node-by-node:** a node flipped to etcd while others are still on the fs lock would split the directory (etcd peers vs fs peers see different sets). `--no-flip` stands etcd up **without** restarting mackesd, so the whole fleet stages first and flips in one fast pass. `--no-files` keeps `/mnt/mesh-storage` on LizardFS (running Syncthing on a live FUSE mount = double replication).

### A1. Founding anchor (the first lighthouse)
```
/usr/libexec/mackesd/cutover-substrate-v2.sh --init --listen <ANCHOR_OVERLAY_IP> --no-flip --no-files
etcdctl --endpoints=http://<ANCHOR_OVERLAY_IP>:2379 endpoint health    # expect: healthy
```

### A2. Every other server / lighthouse
```
/usr/libexec/mackesd/cutover-substrate-v2.sh --join <ANCHOR_OVERLAY_IP> --listen <THIS_OVERLAY_IP> --no-flip --no-files
```

### A3. Every workstation (client-only — no local etcd member)
```
/usr/libexec/mackesd/cutover-substrate-v2.sh --client-only --anchors <ANCHOR1_IP>:2379,<ANCHOR2_IP>:2379 --listen <THIS_OVERLAY_IP> --no-flip --no-files
```

### A4. Verify the cluster BEFORE flipping
```
etcdctl --endpoints=http://<ANCHOR_OVERLAY_IP>:2379 member list   # all server/lighthouse members, started
etcdctl --endpoints=http://<ANCHOR_OVERLAY_IP>:2379 endpoint health --cluster   # all healthy
```
Do **not** proceed if any anchor member is unhealthy.

### A5. FLIP THE FLEET TOGETHER (one fast pass — all nodes)
```
systemctl restart mackesd        # run on EVERY node, as close to simultaneously as practical
```
Verify on a couple of nodes:
```
journalctl -u mackesd --since '2 min ago' | grep -E 'leadership lease \(etcd\)|leader election on etcd'   # leader on etcd, not the lockfile
mackesd peers     # the FULL fleet federates (every node listed, overlay IPs, health)
etcdctl --endpoints=... get --prefix /mesh/peers/ | wc -l   # = 2 × node count (key+value lines)
```
**Rehearsal result:** after the flip, leadership re-acquired on the etcd lease and the directory federated every node. Pre-flip residual `"QNM-Shared … would poison the mountpoint"` WARNs stop once etcd is the active plane (the heartbeat fix); any seen are pre-flip only.

**Phase A is done when** the directory federates the whole fleet over etcd and exactly one node holds the etcd leadership lease. Files are still on LizardFS — untouched.

---

## Phase B — files onto Syncthing (the LizardFS retirement, SUBSTRATE-6) — LATER

Run only after Phase A is stable. Per node (anchors first), drain → unmount LizardFS → plain dir → Syncthing:
```
# stop the LizardFS mount for /mnt/mesh-storage (per the node's qnm-shared unit), confirm it is a PLAIN dir
/usr/libexec/mackesd/setup-syncthing --listen <THIS_OVERLAY_IP>
systemctl enable --now syncthing-reconcile.timer    # auto-armed by setup-syncthing; self-heals the device list
```
The reconcile timer (every 2 min) wires any peer that registered after this node — so order doesn't strand late nodes. Verify:
```
syncthing cli --home=/var/lib/mcnf-syncthing show connections | grep -c '"connected": true'   # = peer count
# write a file on node A's /mnt/mesh-storage → it appears on the others
mesh-health-check.sh    # must NOT emit "Mesh Sync OUT OF SYNC"
```
**Rehearsal result:** file written on A replicated to B over the overlay; the reconcile timer wired devices automatically (no manual step); the out-of-sync health check stayed quiet when connected and fired when a peer dropped.

---

## Verification cheat-sheet (any time, any node)
```
etcdctl --endpoints=http://<overlay-ip>:2379 endpoint health           # coordination plane up
mackesd peers                                                          # directory federates the fleet
journalctl -u mackesd | grep 'leadership lease (etcd)'                 # leader on the lease
systemctl is-active etcd mackesd syncthing nebula                      # services
syncthing cli --home=/var/lib/mcnf-syncthing show connections          # file-plane peers connected
mesh-health-check.sh                                                    # substrate alerts (etcd unreachable / out of sync)
```

## Rollback
- **Phase A back-out (coordination):** on every node `rm -f /etc/mackesd/etcd-endpoints && systemctl restart mackesd` → mackesd resolves the legacy fs path again (its read of the endpoints file is the only gate). Optionally `systemctl stop etcd`.
- **Full back-out:** install the rollback RPM fleet-wide and restart mackesd; the etcd/Syncthing units are inert without the endpoints file.
- **Phase B is the hard one** — once LizardFS is unmounted and files live on Syncthing, rolling back means re-mounting LizardFS and reconciling any files written meanwhile. Do Phase B only after Phase A has soaked.

## Reboot / disconnect self-heal (rehearsed)
Both planes survive a reboot: on the bed, rebooting both nodes brought etcd/mackesd/syncthing/nebula back automatically (boot ordering: `etcd After=nebula`, `mackesd After=etcd`), the 2-member cluster rejoined, leadership re-acquired on the lease, the directory re-federated, and a pre-reboot `/mnt/mesh-storage` file survived + was present on both. No manual steps.

## Bed-rehearsal evidence (2026-06-23, what was validated)
- Onboarding: a fresh node founds-or-joins a reachable mesh from the released RPM (L2 mini-mesh 6/7; the 7th — directory — closes once etcd is provisioned, demonstrated here).
- Coordination: etcd leader election + peer directory federate both nodes (SUBSTRATE-2/3).
- Files: Syncthing replicates `/mnt/mesh-storage` over the overlay; the reconcile timer self-heals the device list (SUBSTRATE-5).
- Recovery: reboot self-heal of every service + the file plane (SUBSTRATE-7/14).
- Alerting: "etcd unreachable" + "Mesh Sync OUT OF SYNC" fire on real fault injection (SUBSTRATE-10/11).
