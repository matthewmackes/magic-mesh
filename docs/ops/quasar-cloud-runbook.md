# CONSTRUCT-CLOUD operator runbook — standing up and running the mesh cloud

> **Status: tracks the LOCKED design** (`docs/design/quasar-cloud.md`,
> 2026-07-03) and is written as the QC-21 docs deliverable while the build-out
> (QC-1..QC-20) lands. Steps that depend on an unshipped task carry that task
> id; anything the design leaves open says **decided at implementation** rather
> than guessing. Update this file as each task ships — QC-21's acceptance is
> that the docs match the shipped behavior.
>
> **Provider-neutral runway note, 2026-07-18:** the OpenStack/Kolla deployment
> below remains the installed backend until replacement-provider proof exists,
> but it is now an adapter behind Construct Cloud contracts. New operator
> surfaces, Bus verbs, persisted mirrors, and docs should prefer provider-neutral
> names and keep OpenStack terms in compatibility diagnostics.

Every MCNF node is a **universal OpenStack node**: the one bootc image carries
the host virt bits, and a mackesd `openstack` worker runs Red-Hat-convention
**Kolla service containers under Podman** on any node the fleet state says.
There is **no controller box** — APIs run on every OpenStack-carrying node, and
the control plane is distributed (leader-hosted MariaDB, clustered RabbitMQ,
tooz on the mesh etcd). All API traffic is plaintext **bound to the Nebula
overlay interface only — Nebula is the transport security** (Q23). Red Hat
*conventions* (SELinux, systemd/Quadlet, OpenSCAP discipline) are the standard;
the deployment machinery is mesh-native (fleet state rendered into Kolla
config), not TripleO/RHOSO (Q3).

## 1. What runs where

| Layer | Component | Where | Notes |
|---|---|---|---|
| Host image | libvirt/QEMU-KVM, OVN, kernel modules | every node | baked into the one bootc image (QC-1, Q11/12); cloud-hypervisor is absent |
| Host image | `python-openstackclient` | every node | the CLI, ships in the image (Q27) |
| Supervisor | mackesd `openstack` worker | every OpenStack-carrying node | renders Kolla config from fleet state, owns the Podman units, reports per-service health on the Bus (Q20/30, QC-2) |
| MVP APIs | Keystone, Nova + Placement, Neutron (ML2/OVN), Glance, Cinder | **every** OpenStack-carrying node | API containers bound plaintext to the Nebula interface (Q22/23/24, QC-6) |
| Wave 2 | Heat (rendered from fleet state), Designate, Octavia, optional Horizon | per fleet state | Q25/61/46/47; Horizon, if deployed, is mesh-only |
| Database | MariaDB | **the etcd leader** | a workload that re-places on leader failover (Q15, QC-4) |
| Messaging | RabbitMQ, quorum queues | clustered | **OpenStack-internal RPC only** — mde-bus stays THE platform bus (Q16/67) |
| Coordination | tooz → mesh etcd; memcached | etcd mesh-wide; memcached per node | Q17 |
| Block storage | Cinder LVM | per node (volumes node-local) | carved from the writable partition alongside the Swift dir + Nova ephemeral space (Q51/59) |
| Images | Glance, local file store + replication/caching between API nodes | API nodes | fed by the diskimage-builder pipeline (Q36/53, QC-9) |
| Object storage | Swift (hot tier) + DO Spaces (off-site) | ring-based, no center | cinder-backups land here (Q54/55/57, QC-18) |

Which nodes carry OpenStack duties is declared in fleet state (one-state
doctrine: etcd + TOML-on-Syncthing, Q30). The exact fleet-state keys and verbs
are **decided at implementation** (QC-2).

## 2. Leader placement and failover

MariaDB is **leader-hosted**: it rides the etcd leader and re-places when
leadership moves (Q15). Accepted consequence: a **brief control-plane write
outage on failover** — running instances and workloads keep running; only API
writes stall until the DB re-places.

The acceptance drill (design acceptance #3): kill the node hosting MariaDB —
the etcd leader moves, the DB workload re-places, the APIs recover. **No
permanently-special node.** Run this drill on the farm-VM dev cloud before
trusting it live.

## 3. Standing up the cloud

1. **Prove convergence on the dev cloud first.** Disposable farm VMs act as
   virtual mesh nodes, IaC'd (Q75, QC-16). "Everywhere at once" (Q71) is the
   hardest bring-up mode; the dev cloud is the mitigation.
2. **Mirror the Kolla images onto the mesh** (§4 below) — deploys must work
   offline, no registry.
3. **Declare the cloud in fleet state.** The fleet state declares which nodes
   carry which services; **every node converges together** (Q71) — there is no
   node-by-node bring-up. The mackesd `openstack` worker on each node
   materializes the Podman units and Kolla config (QC-2). Exact declaration
   format: **decided at implementation** (QC-2).
4. **Verify:** per-service health green on the Bus from every carrying node;
   `openstack endpoint list` resolves to a mesh name reaching any healthy node;
   APIs unreachable from non-Nebula interfaces (QC-6).

## 4. Kolla image mirror (the airgap lane)

There is **no registry** (Q18). Service images travel the mesh as archives on
the Syncthing share and are loaded locally:

1. **Pin one Kolla release** — whatever installs clean on the Fedora base
   (CentOS-Stream-based Kolla images, Q4/19). Stay pinned until CVEs/EOL force
   a move (Q69).
2. On a connected host, pull the pinned images and save them as archives
   (`podman save`), with checksums.
3. Drop the archives on `/mnt/mesh-storage` (the Syncthing lane) — Syncthing
   replicates them to every node (QC-3).
4. Each node's `openstack` worker verifies checksums and `podman load`s
   locally. **No `quay.io` (or any registry) pull at deploy time** (QC-3
   acceptance).

Archive directory layout and naming: **decided at implementation** (QC-3).

## 5. Network facts operators must know

- **Nebula stays the substrate** (Q41) — identity, WAN traversal, NAT punch.
  OVN rides on top of it.
- **One flat provider network bridged into the mesh** (Q43): every instance is
  peer-equivalent — "inside" with **no per-instance Nebula certs** (Q44).
- **Default-open security groups inside** (Q45); the **host firewalld keeps the
  public boundary**. The blast-radius consequences are documented in
  `DISCLAIMER.md` ("Blast radius").
- **MTU:** Geneve-over-Nebula double encapsulation is accepted; the tenant MTU
  must be set correctly (**~1342**) (Q49). Symptom of getting this wrong:
  small packets pass, large transfers/TLS handshakes inside instances stall.
- **Floating IPs come from each site's LAN** (Q48).
- **IPv4-only** (Q50).
- **DNS:** endpoints resolve via Nebula-DNS/peer-directory (Q22); wave 2 hands
  naming to **Designate**, fed by the peer directory — which remains the source
  that can **re-seed Designate from scratch** if it is lost (Q46, QC-17).
- **Ingress:** Octavia serves *instance* workloads; **Lighthouse Caddy keeps
  platform ingress** (Q47).

## 6. Identity and quotas

- **The mesh account IS the cloud account** (Q81): enrollment provisions
  Keystone users + app credentials (Q21); tokens are minted invisibly (Q87) —
  no login step anywhere.
- **Single tenant** — one domain/project (Q7).
- The mesh **CA/KDC narrows to machine certs**; Keystone absorbs human users
  (Q62, QC-5).
- **Flavors and quotas are capacity-derived** from real node shapes/capacity
  (Q29/39); quotas are **hard per-user Keystone limits** — the mesh's first
  hard authorization boundary (Q89, QC-10). Exceeding one is rejected by
  Keystone/Nova and surfaced in the UI.

## 7. Health and monitoring

- The mackesd `openstack` worker publishes **per-service health on the Bus**;
  a killed container restarts; `[!]`-grade failures surface in chat (QC-2).
- **netdata + mesh-health stay**, extended with cloud checks (Q64).
- **OpenStack notifications fold into mesh chat**: services appear as roster
  contacts; an instance failure arrives as a signed chat message from the
  service contact (Q65, QC-20).
- **Idle instances nudge their owners** in chat — no auto-delete (Q90).

## 8. Troubleshooting

| Symptom | Check |
|---|---|
| API unreachable from a node | The overlay first: APIs bind **only** to the Nebula interface (Q23). No `nebula1`/overlay IP ⇒ no cloud API. |
| API writes erroring, reads fine | Leader failover in progress — MariaDB is re-placing (Q15). Expected to self-heal; verify the etcd leader and the DB workload placement. |
| Instance reachable for pings, stalls on bulk transfer | Tenant MTU. Must be ~1342 for Geneve-over-Nebula (Q49). |
| Names stop resolving (wave 2) | Designate rides the cloud's availability. The peer directory can re-seed it from scratch (Q46). |
| A service container keeps dying | The worker restarts it and reports on the Bus (QC-2); check the per-service health lane and the container logs via Podman. |
| Deploy tries to reach a registry | Broken mirror lane — deploys must load from `/mnt/mesh-storage` archives only (QC-3, §4 above). |
| AVC denials from OpenStack domains | Expected initially: OpenStack service domains run **permissive** at first (Q14). Record them — they feed the QC-22 tightening; do not silently `setenforce 0` the host, which stays enforcing. |

## 9. SELinux posture

The host is enforcing; **OpenStack service domains start permissive** (Q14).
This is a tracked hardening gap, not an accepted end state: QC-22 moves the
domains to enforcing with proper policy modules after a clean soak. Until then,
collect AVCs (see §8) rather than working around them.

## 10. Upgrades

**Pin until forced** (Q69): stay on the pinned Kolla release until CVEs or EOL
force a move. An upgrade is a new mirrored archive set (§4) plus a fleet-state
change — procedure **decided at implementation** when the first forced upgrade
arrives.

## Related documents

- `docs/design/quasar-cloud.md` — the locked design (90-Q survey, lock table).
- `docs/ops/quasar-cloud-cutover-map.md` — what dies at cutover and what
  replaces it.
- `docs/help/cloud-self-service.md` — the member how-to for the Cloud plane.
- `DISCLAIMER.md` — the blast radius, extended for cloud instances.
- `docs/help/mesh-recovery.md` — mesh-level DR (unchanged by this epic).
