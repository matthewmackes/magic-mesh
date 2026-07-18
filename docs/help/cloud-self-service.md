# Cloud Self-Service — your instances from the Cloud plane

Every mesh member can run their own virtual machines on the mesh cloud. Your
mesh account **is** your cloud account — there is no separate login, no token
to paste, nothing to configure (invisible SSO). Everything happens in the
Workbench's **Cloud plane** (it replaced the old Controller plane), and it is
**mesh-only**: there is no web portal, and nothing is exposed to the internet.
For access from outside the mesh, use a VDI desktop.

> **Status:** this guide tracks the locked CONSTRUCT-CLOUD design
> (`docs/design/quasar-cloud.md`). The Cloud plane ships with QC-12/QC-13;
> exact widget layout is decided at implementation. One plane serves everyone —
> operators and members alike; there is no separate "My Cloud".

## "Instance" vs "VM": which panel owns your machine

The shell has two VM surfaces over the same libvirt/QEMU-KVM hosts, and they use
different nouns on purpose:

- **Cloud plane → *instances*.** This is the self-service cloud (Nova/OpenStack).
  If you launched it here, manage it here — boot/stop/rebuild/delete, volumes,
  snapshots, and Heat stacks. This is the surface for your own machines.
- **Fleet ▸ Datacenter → *VMs*.** This is the raw per-node libvirt view (an
  operator/infrastructure lens onto every domain each host runs). It exists for
  local, unmanaged domains and for seeing what physically runs where.

**Ownership rule:** a Nova-launched instance is also a libvirt domain, so it can
appear in **both** panels. The Cloud plane owns its lifecycle; treat such
Nova-managed domains as **read-only in Fleet ▸ Datacenter** and drive them from
the Cloud plane. Datacenter directly owns only unmanaged/local domains.

> The engineering unification of the two code paths and any UI-string / badging
> cleanup (marking Nova-managed rows in Datacenter, cross-linking the two panels)
> is tracked as a follow-up — see the naming/UI note in
> [`docs/NEEDS-OPERATOR.md`](../NEEDS-OPERATOR.md).

## Launch an instance

Open the Cloud plane and start the launch picker. It walks four choices:

1. **Image** — the base OS, from the mesh's Glance catalog.
2. **Flavor** — the size. Flavors are generated from the real shapes of the
   mesh's nodes, so what you see is what the hardware can actually give.
3. **Network** — instances land on the one flat mesh network (see below).
4. **Volume** — optionally attach a data volume at launch.

Saved **templates** appear as launch presets — they are fleet-state records any
node can author, so a preset made anywhere shows up everywhere.

## Get into your instance

- **Console:** the instance row requests Nova's SPICE console descriptor. Direct
  `spice://host:port` descriptors open in the native Desktop viewer; Nova HTML5
  proxy URLs are shown as gated because they are not raw SPICE sockets.
- **SSH:** launches use the mesh-derived Nova keypair `mcnf-mesh`; the daemon
  ensures that keypair exists before Heat creates the server.

## What you can manage

All five resource kinds, from the same plane:

- **Instances** — boot, stop, rebuild, delete.
- **Volumes + snapshots** — create, attach, detach, snapshot.
- **Images** — the catalog you launch from.
- **Networks + stacks** — stacks arrive with the wave-2 services (Heat).

The plane also shows **your usage** against your quota.

## Keep your data safe

**Root disks are ephemeral by default** — a rebuild or delete loses whatever is
on them. Put anything that must survive on a **volume** and attach it; volumes
outlive instances. Snapshot volumes for point-in-time copies. Volume backups
land in the mesh's object store (arrives with QC-18).

## Networking: your instance is "inside"

Your instance joins the mesh's flat network as a peer-equivalent: **every mesh
peer can reach it, and it can reach every peer** — the default inside the mesh
is open. There is no per-instance firewall configuration to fight, and also
none protecting your instance from other mesh machines (or them from it). Boot
only images you trust, keep it updated, and see the "Blast radius" section of
`DISCLAIMER.md` for what this trade-off means. IPv4 only.

## Quotas

Your quota is a **hard per-user limit**, derived from the mesh's real capacity.
Exceeding it is rejected outright and shown in the UI. If you need more, free
resources you are not using — or ask the operator.

## Idle instances

Nothing you leave running is auto-deleted. An idle instance sends you a **chat
nudge** so you can decide: keep it, snapshot it, or delete it.

## CLI

Prefer a terminal? `openstack` (python-openstackclient) is on every node and
shows exactly the same state as the plane — every Cloud-plane action goes
through the same typed mesh verbs.
