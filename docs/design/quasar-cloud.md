# QUASAR-CLOUD — the mesh becomes an OpenStack cloud (universal node)

**Status:** LOCKED 2026-07-03 (90-Q `/plan` survey, operator).
**Prompt:** *"What would it take to make this platform into a universal OpenStack
node, based on Red Hat standards"* — redirected mid-survey by the operator:
**"I would like all of these features to replace the methods in the platform
today when possible."** This is therefore a **replacement epic**, not an
add-on: where OpenStack has a mature counterpart to a platform method, the
OpenStack service becomes the method.
**Supersedes:** `mesh-virt-management.md` pillar 1 (cloud-hypervisor) and its
"mesh-native #5" scheduler lock · the Cockpit-interim console · the §9
Controller plane framing · per-instance mesh certs (the VDI dual-homing
precedent does not extend to cloud instances).
**Companions:** `quasar-vdi-desktop.md` (the shell/VDI premise — unchanged),
`onboarding-wizard.md` (2-role model — unchanged), `enterprise-readiness.md`
(ENT-12 blast-radius doc — extended here).

## One-paragraph architecture

Every MCNF node is a **universal OpenStack node**: the one bootc image carries
the host virt bits (libvirt/QEMU-KVM, OVN) and a **mackesd `openstack` worker**
that runs Red-Hat-convention **Kolla service containers** under Podman on any
node the fleet state says. The control plane is **distributed — APIs on every
node, no controller box** (leader-hosted MariaDB, clustered RabbitMQ, tooz on
the mesh etcd), bound plaintext to the **Nebula overlay, which is the transport
security**. **Nova+Placement replace the mesh-native VM scheduler**;
libvirt/QEMU replaces cloud-hypervisor; Glance+DIB replace the golden-image
script; Cinder (LVM) adds volumes; Neutron/OVN puts every instance on **one
flat provider network bridged into the mesh** — instances are "inside" without
their own Nebula certs. Designate replaces DNS/naming, Swift + DO Spaces form
a two-tier object store, and the Workbench's **Controller plane becomes the
Cloud plane**, where every mesh member self-serves through typed mackesd verbs
that wrap the OpenStack APIs. Red Hat *conventions* (SELinux, systemd/Quadlet,
OpenSCAP discipline) are the standard; the deployment machinery is mesh-native
(one-state doctrine → rendered Kolla config), not TripleO/RHOSO.

## Lock table (90 questions, 9 rounds)

### Round 1 — Scope & positioning (Q1–10)
| # | Decision | Lock |
|---|---|---|
| 1 | Node scope | **Any-role node** — one image serves controller/compute/storage/network duties by config |
| 2 | Driver | **Own private cloud** for the workgroup |
| 3 | "Red Hat standards" | **Conventions only** — SELinux/systemd/OpenSCAP/FHS discipline; deploy mesh-native, not RHOSO/TripleO |
| 4 | Release | **Whatever installs clean** on the Fedora base (via Kolla) |
| 5 | §0 vs control plane | **Distributed everywhere** — clustered control services, no controller box |
| 6 | Role model | **Pure workload** — OpenStack services are scheduled workloads; the 2-role lock stands |
| 7 | Tenancy | **Single tenant** (one domain/project) |
| 8 | Scale | **Raise §8 for compute** — control small, compute nodes to dozens (carve-out like the VDI one) |
| 9 | Build farm | **Untouched** — the farm builds the platform; OpenStack deploys over the mesh between node machines, no center |
| 10 | Sequencing | **Parallel epic** alongside E12 (daemon/infra-side, file-disjoint from GUI) |

### Round 2 — Base OS, packaging & foundations (Q11–20)
| # | Decision | Lock |
|---|---|---|
| 11 | Packaging | **Kolla containers** under Podman |
| 12 | Immutability | **Host bits in the image** (libvirt/QEMU, OVN, kernel modules); services containerized |
| 13 | §3 OpenSSL lock | **Our code only** — hosted workloads bring their own crypto (governance clarified) |
| 14 | SELinux | Enforcing host; **permissive domains for OpenStack services initially**, tighten later |
| 15 | Database | **Leader-hosted MariaDB** — a workload on the etcd leader, re-placed on failover |
| 16 | Messaging | **Clustered RabbitMQ** with quorum queues |
| 17 | Coordination | **tooz on the mesh etcd**; memcached per node |
| 18 | Image transport | **Syncthing-distributed** archives + `podman load` — no registry |
| 19 | Distro fit | **Containers solve it** — CentOS-Stream-based Kolla images on the Fedora host |
| 20 | Supervision | **mackesd `openstack` worker** owns the Podman units, reports over the Bus |

### Round 3 — Identity, APIs & services (Q21–30)
| # | Decision | Lock |
|---|---|---|
| 21 | Keystone backend | **Bridge mesh identity** — enrollment provisions Keystone users + app credentials |
| 22 | Endpoints | **APIs on every node**; Nebula-DNS/peer-directory resolution |
| 23 | API TLS | **Nebula is the TLS** — plaintext HTTP bound to the overlay interface only |
| 24 | MVP services | **Nova+Placement, Neutron, Glance, Cinder** (+Keystone) |
| 25 | Wave 2 | **Horizon, Heat, Designate + Octavia** (no Barbican — Q63) |
| 26 | Cloud UI | **Workbench first-class** (egui Cloud plane over mackesd verbs); Horizon optional |
| 27 | CLI | **python-openstackclient in the host image** |
| 28 | Topology | **One region, one cell** |
| 29 | Quotas | **Capacity-derived** (auto from real mesh capacity) — hardened per-user by Q89 |
| 30 | State home | **One-state doctrine** — etcd + TOML-on-Syncthing rendered into Kolla config by the worker |

### Round 4 — Compute replacement (Q31–40)
| # | Decision | Lock |
|---|---|---|
| 31 | Nova verdict | **Full replacement** — Nova+Placement own ALL VM lifecycle; the mesh-native scheduler lock is superseded |
| 32 | Hypervisor | **libvirt/QEMU-KVM**; cloud-hypervisor retires; virtio-gpu→egui path re-validated on QEMU |
| 33 | VDI | **Nova + broker overlay** — Nova owns the instance; session-broker layers display path/roaming/seat binding |
| 34 | Consoles | **SPICE + the mde-vdi-spice egui viewer** as THE console experience |
| 35 | Containers | **Keep the Podman stack** — no Zun/Magnum; OpenStack replaces VM methods only |
| 36 | Images | **Glance + diskimage-builder pipeline** replace `build-mde-vm-golden.sh` and on-disk qcow2s |
| 37 | Passthrough | **Nova PCI + vGPU flavors** model GPU/device passthrough |
| 38 | Migration | **Not a goal** — roaming = rebuild from image/volume; no live migration |
| 39 | Flavors | **Capacity-derived** — generated from real node shapes |
| 40 | Verbs | **Verbs wrap Nova** — typed Bus verbs stay the contract; OpenStack is the backend. §9 holds |

### Round 5 — Networking replacement (Q41–50)
| # | Decision | Lock |
|---|---|---|
| 41 | Nebula | **Stays the substrate** — identity/WAN/NAT-punch; OVN rides on top. §1 stands |
| 42 | Neutron driver | **ML2/OVN** |
| 43 | Net model | **One flat provider network bridged into the mesh** — every instance a peer-equivalent |
| 44 | Guest certs | **None** — instances connect via Neutron and are "inside"; no per-instance Nebula certs (VDI-guest precedent narrowed) |
| 45 | Firewalling | **Default-open inside** (permissive default security group); host firewalld keeps the public boundary |
| 46 | DNS | **Designate replaces DNS/naming** — the peer directory feeds it; nodes, instances, services get records |
| 47 | Ingress/LB | **Octavia for instance workloads**; Lighthouse Caddy keeps platform ingress |
| 48 | Floating IPs | **FIPs from each site's LAN** |
| 49 | Encap/MTU | **Accept Geneve-over-Nebula double encap**; set tenant MTU correctly (~1342) |
| 50 | IPv6 | **IPv4-only, documented** |

### Round 6 — Storage replacement (Q51–60)
| # | Decision | Lock |
|---|---|---|
| 51 | Cinder backend | **LVM per node** (volumes node-local; fits no-live-migration) |
| 52 | Syncthing | **Stays** — SUBSTRATE-V2 stands; cloud storage is additive |
| 53 | Glance store | **Local file + Glance replication/caching** between API nodes |
| 54 | Object storage | **Both tiers** — self-hosted hot + DO Spaces off-site |
| 55 | Hot-tier impl | **Swift proper** (ring-based, no-center by design, Keystone-native) |
| 56 | Root disks | **Ephemeral default**; Cinder volumes for data that must survive |
| 57 | Backups | **cinder-backup to the object tiers** |
| 58 | Existing VMs | **Fresh start** — rebuild, don't import |
| 59 | Disk layout | **Carve the writable partition** — LVM VG + Swift dir + Nova ephemeral |
| 60 | Media stack | **Becomes instances** — Navidrome + media re-platform onto the cloud (first proof) |

### Round 7 — Orchestration, identity & observability (Q61–70)
| # | Decision | Lock |
|---|---|---|
| 61 | Heat vs fleet | **Fleet renders Heat** — etcd/Syncthing state authoritative; the worker renders stacks; Heat executes |
| 62 | Identity split | **Keystone absorbs human users**; the mesh CA/KDC narrows to machine certs |
| 63 | Barbican | **Skip** — revisit when a service demands it |
| 64 | Telemetry | **Keep netdata + mesh-health**, extended with cloud checks |
| 65 | Alerts | **Mesh chat contacts** (NOTIFY-CHAT) — a mackesd worker folds OpenStack notifications into the chat lanes |
| 66 | Cockpit | **Retire with Nova** — Workbench (+ optional Horizon) are the consoles |
| 67 | Bus split | **Strict** — RabbitMQ is OpenStack-internal RPC only; mde-bus stays THE platform bus (§2 untouched) |
| 68 | Ironic | **Skip** — the ISO + role-chooser stays the enrollment path |
| 69 | Upgrades | **Pin until forced** (CVEs/EOL) |
| 70 | Workbench IA | **The Controller plane BECOMES the Cloud plane** — OpenStack is the control brain §9 described |

### Round 8 — Migration, testing & program (Q71–80)
| # | Decision | Lock |
|---|---|---|
| 71 | Rollout | **Everywhere at once** — fleet state declares the cloud; every node converges together |
| 72 | Coexistence | **Hard cutover** per node — old-stack or Nova, never both |
| 73 | Replaced code | **Delete on cutover**, same-epic (§7 tolerates no dead code) |
| 74 | CI proof | **Own verb-level tests** — Rust integration tests through the mackesd verbs (boot/net/volume round-trips) |
| 75 | Dev cloud | **Farm VMs as virtual mesh nodes** — disposable, IaC'd |
| 76 | Packaging | **Host bits + worker in the image**; Kolla images via the Syncthing lane |
| 77 | Docs | **All four** — operator guide · ENT-12 blast-radius update · architecture (replaced-vs-kept) map · user how-to |
| 78 | Epic identity | **QUASAR-CLOUD**, tasks `QC-N`, this file |
| 79 | MVP bar | **Boot-attach-reach** — from the Workbench Cloud plane, boot from Glance, attach Cinder, reach from any peer, all through verbs |
| 80 | Governance | **Amend now** — AI_GOVERNANCE.md updated alongside this doc |

### Round 9 — End-user self-service (Q81–90)
| # | Decision | Lock |
|---|---|---|
| 81 | End user | **Every mesh member** — the mesh account IS the cloud account |
| 82 | Surface | **One Cloud plane for all** (admin + self-service; no separate "My Cloud") |
| 83 | Launch UX | **Full picker** — image/flavor/network/volume wizard |
| 84 | Templates | **Fleet-state records any node can author** (§0) |
| 85 | Scope | **Everything** — instances · volumes+snapshots · images · networks+stacks |
| 86 | Access | **Mesh only** — no web exposure; remote access converges on VDI |
| 87 | Auth UX | **Invisible SSO** — mesh identity mints Keystone tokens automatically |
| 88 | Instance access | **Console + keys** — one-click SPICE console AND automatic SSH key injection |
| 89 | Guardrails | **Hard per-user Keystone quotas** — the first hard authorization boundary inside the mesh (documented §9 no-RBAC departure) |
| 90 | Hygiene | **Idle nudges** — no auto-delete; idle instances trigger a chat nudge to the owner |

## Replaced-vs-kept map (the Q-comparison, maintained)

| Domain | Was | Becomes | Verdict |
|---|---|---|---|
| VM lifecycle/scheduling | mackesd vm-lifecycle + mesh-native scheduler | **Nova + Placement** | REPLACED (Q31) |
| Hypervisor | cloud-hypervisor | **libvirt/QEMU-KVM** | REPLACED (Q32) |
| VM golden images | `build-mde-vm-golden.sh` + qcow2s | **Glance + DIB** | REPLACED (Q36) |
| VM consoles | ad-hoc | **SPICE via mde-vdi-spice** | REPLACED (Q34) |
| Block storage | raw disks on writable | **Cinder LVM** | REPLACED (Q51) |
| DNS/naming | peer directory + hostnames | **Designate** (peer directory feeds it) | REPLACED (Q46) |
| VM web console | Cockpit (interim) | **Cloud plane (+Horizon)** | REPLACED (Q66) |
| Human identity | mesh CA/KDC accounts | **Keystone** (CA/KDC → machine certs only) | REPLACED (Q62) |
| Cloud orchestration | — | **Heat, rendered from fleet state** | NEW, fleet-authoritative (Q61) |
| Object/media store | DO Spaces only | **Swift hot + DO Spaces off-site**; media → instances | REPLACED/two-tier (Q54/55/60) |
| Instance ingress/LB | — | **Octavia** (platform ingress keeps Caddy) | SPLIT (Q47) |
| Node-to-node fabric | Nebula | Nebula (OVN on top) | **KEPT** (Q41) |
| Coordination | etcd | etcd (+tooz) | **KEPT** (Q17) |
| File sync | Syncthing SUBSTRATE-V2 | Syncthing | **KEPT** (Q52) |
| Containers | Podman + mackesd | Podman + mackesd | **KEPT** (Q35) |
| Platform bus | mde-bus | mde-bus (RabbitMQ internal-only) | **KEPT** (Q67) |
| Telemetry | netdata + mesh-health | same, + cloud checks | **KEPT** (Q64) |
| Notifications | mesh chat (NOTIFY-CHAT) | same, + cloud contacts | **KEPT** (Q65) |
| Enrollment/install | ISO + role chooser | same (no Ironic) | **KEPT** (Q68) |
| Typed verbs (§9) | mackesd Bus verbs | same, wrapping OpenStack | **KEPT** (Q40) |

## Acceptance criteria (MVP — runtime-observable)

1. **Boot-attach-reach (Q79):** from the Workbench Cloud plane on any node, a
   member boots an instance from a Glance image, attaches a Cinder volume, and
   reaches the instance from a different mesh peer over the flat provider net —
   every step through mackesd verbs; `openstack` CLI shows the same state.
2. The fleet state declares the cloud; **every node converges** (Q71) with the
   mackesd openstack worker reporting per-service health on the Bus.
3. Kill the node hosting MariaDB: the leader moves, the DB workload re-places,
   APIs recover — **no permanently-special node** (Q5/Q15).
4. All API traffic observed only on the Nebula interface (Q23); the public
   boundary posture is unchanged (`CONNECT` tiers hold).
5. Keystone contains exactly the enrolled members (invisible SSO, Q87);
   per-user quotas enforce (Q89); an idle instance produces a chat nudge (Q90).
6. The old stack is gone on cutover nodes: no cloud-hypervisor binary in use,
   no Cockpit VM console, `build-mde-vm-golden.sh` deleted — `/audit` clean (Q73).
7. Verb-level integration tests (boot/net/volume round-trip) run in CI against
   the farm-VM dev cloud (Q74/Q75).

## Risks (accepted, eyes-open)

- **"Everywhere at once" first light (Q71)** is the hardest bring-up mode;
  mitigated by the farm-VM dev cloud proving the convergence path first.
- **Leader-hosted MariaDB (Q15)** = brief control-plane write outages on
  failover; accepted at workgroup scale (workloads keep running).
- **Kolla-without-kolla-ansible:** rendering Kolla config from fleet state
  (Q30) re-implements a slice of kolla-ansible; contained by pinning one
  release (Q69).
- **Permissive SELinux domains (Q14)** weaken the RH-conventions claim until
  tightened — tracked as a hardening task, not silently deferred.
- **Designate as THE name service (Q46)** puts DNS on the cloud's availability;
  the peer directory remains the source that can re-seed it.
- **virtio-gpu→egui on QEMU (Q32)** must be re-validated; the VDI fast path was
  built on cloud-hypervisor.
- **Hard per-user quotas (Q89)** introduce the mesh's first hard authz
  boundary — a deliberate §9 departure, documented in governance.

## Out of scope (explicit)

Multi-tenant/customer isolation (Q7) · datacenter scale (Q8) · farm migration
(Q9) · live migration (Q38) · Ceph (Q51 — LVM chosen; Ceph would be its own
epic) · Barbican (Q63) · Ironic (Q68) · Aodh/Ceilometer (Q64/65) ·
Zun/Magnum (Q35) · IPv6 (Q50) · web-exposed self-service (Q86) ·
Horizon-as-primary (Q26).
